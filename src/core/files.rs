//! Fichiers envoyés (`nf_file` + dossier `upload/`), porté de `neofrag/models/file.php`.
//!
//! Seules les images sont acceptées pour l'instant. Par rapport à l'original :
//! - le type est déterminé par le contenu du fichier, jamais par son nom ni par le
//!   `Content-Type` annoncé ; l'extension enregistrée découle de ce type ;
//! - l'image est décodée puis ré-encodée : cela supprime les métadonnées (position GPS
//!   des photos…) et neutralise les fichiers piégés qui ne sont images qu'en apparence ;
//! - les SVG sont refusés (ils peuvent contenir du script).

use super::{ctx::Ctx, password, text};
use crate::error::AppError;
use axum::extract::Multipart;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use sqlx::Row;
use std::{collections::HashMap, io::Cursor, path::PathBuf};

/// Un fichier reçu dans un formulaire.
pub struct Upload {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Contenu d'un formulaire `multipart/form-data`.
#[derive(Default)]
pub struct FormData {
    fields: HashMap<String, String>,
    files: HashMap<String, Upload>,
}

impl FormData {
    pub fn text(&self, name: &str) -> &str {
        self.fields.get(name).map(String::as_str).unwrap_or("")
    }

    pub fn checked(&self, name: &str) -> bool {
        self.fields.contains_key(name)
    }

    pub fn take_file(&mut self, name: &str) -> Option<Upload> {
        self.files.remove(name)
    }
}

/// Lit un formulaire multipart. La taille totale est bornée par la limite de corps de
/// la route (`DefaultBodyLimit`) ; un dépassement donne l'erreur « trop volumineux ».
pub async fn read_multipart(mut multipart: Multipart) -> Result<FormData, &'static str> {
    const TOO_BIG: &str = "Formulaire invalide ou fichier trop volumineux.";
    let mut data = FormData::default();
    while let Some(field) = multipart.next_field().await.map_err(|_| TOO_BIG)? {
        let Some(name) = field.name().map(str::to_owned) else {
            continue;
        };
        match field.file_name().map(str::to_owned) {
            Some(file_name) => {
                let bytes = field.bytes().await.map_err(|_| TOO_BIG)?;
                // Champ fichier laissé vide par le navigateur.
                if !bytes.is_empty() {
                    data.files.insert(name, Upload { name: file_name, bytes: bytes.to_vec() });
                }
            }
            None => {
                data.fields.insert(name, field.text().await.map_err(|_| TOO_BIG)?);
            }
        }
    }
    Ok(data)
}

/// Mise aux dimensions appliquée à une image envoyée.
#[derive(Clone, Copy)]
pub enum Fit {
    /// Recadrée au centre en carré de N pixels (avatars).
    Square(u32),
    /// Réduite si elle dépasse N pixels de large (illustrations).
    MaxWidth(u32),
}

const NOT_AN_IMAGE: &str = "Ce fichier n'est pas une image valide (formats acceptés : JPEG, PNG, GIF, WebP).";

/// Décode, redimensionne et ré-encode. Renvoie les octets et l'extension.
fn process(bytes: &[u8], fit: Fit) -> Result<(Vec<u8>, &'static str), &'static str> {
    let format = image::guess_format(bytes).map_err(|_| NOT_AN_IMAGE)?;
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Gif | ImageFormat::WebP) {
        return Err(NOT_AN_IMAGE);
    }

    // Bornes de décodage : une image minuscule sur le disque peut en décrire une gigantesque.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(10_000);
    limits.max_image_height = Some(10_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);

    let mut decoder = reader.into_decoder().map_err(|_| NOT_AN_IMAGE)?;
    // Les photos de téléphone sont souvent stockées couchées avec une consigne de rotation.
    let orientation = decoder.orientation().map_err(|_| NOT_AN_IMAGE)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|_| NOT_AN_IMAGE)?;
    img.apply_orientation(orientation);

    let img = match fit {
        Fit::Square(size) => img.resize_to_fill(size, size, FilterType::Lanczos3),
        Fit::MaxWidth(width) if img.width() > width => img.resize(width, img.height(), FilterType::Lanczos3),
        Fit::MaxWidth(_) => img,
    };

    let mut out = Vec::new();
    if format == ImageFormat::Jpeg {
        let rgb = img.to_rgb8();
        JpegEncoder::new_with_quality(&mut out, 88)
            .encode_image(&rgb)
            .map_err(|_| NOT_AN_IMAGE)?;
        Ok((out, "jpg"))
    } else {
        // PNG, GIF et WebP peuvent être transparents : on garde un format qui le permet.
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .map_err(|_| NOT_AN_IMAGE)?;
        Ok((out, "png"))
    }
}

/// Chemin sur le disque d'un `nf_file.path` (`upload/news/x.jpg`), ou `None` si le
/// chemin en base ne désigne pas un fichier du dossier d'envoi.
fn disk_path(ctx: &Ctx, db_path: &str) -> Option<PathBuf> {
    relative_upload_path(db_path).map(|relative| ctx.state.config.site.upload_dir.join(relative))
}

fn relative_upload_path(db_path: &str) -> Option<&str> {
    // Les anciennes données écrivent parfois `./upload/…`.
    let relative = db_path.trim_start_matches("./").strip_prefix("upload/")?;
    let safe = !relative.is_empty()
        && relative
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\\'));
    safe.then_some(relative)
}

/// Enregistre une image envoyée dans `upload/<dir>/` et dans `nf_file`.
/// Renvoie l'identifiant du fichier, ou un message à afficher à l'utilisateur.
pub async fn save_image(ctx: &Ctx, dir: &'static str, upload: Upload, fit: Fit) -> Result<Result<u32, &'static str>, AppError> {
    let max_bytes = ctx.state.config.site.max_upload_mb.saturating_mul(1024 * 1024);
    if upload.bytes.len() > max_bytes {
        return Ok(Err("Ce fichier est trop volumineux."));
    }

    let Upload { name, bytes } = upload;
    // Le décodage est coûteux : hors du fil d'exécution asynchrone.
    let processed = tokio::task::spawn_blocking(move || process(&bytes, fit))
        .await
        .map_err(|_| AppError::Internal("traitement d'image interrompu"))?;
    let (bytes, extension) = match processed {
        Ok(result) => result,
        Err(message) => return Ok(Err(message)),
    };

    let file_name = format!("{}.{extension}", password::unique_id());
    let db_path = format!("upload/{dir}/{file_name}");
    let folder = ctx.state.config.site.upload_dir.join(dir);
    tokio::fs::create_dir_all(&folder).await.map_err(AppError::Io)?;
    tokio::fs::write(folder.join(&file_name), &bytes).await.map_err(AppError::Io)?;

    // Nom d'origine : uniquement informatif, réduit à son dernier segment.
    let original = name.rsplit(['/', '\\']).next().unwrap_or("");
    let original: String = text::encode(original).chars().take(100).collect();

    let inserted = sqlx::query("INSERT INTO nf_file (user_id, name, path) VALUES (?, ?, ?)")
        .bind(ctx.user.as_ref().map(|u| u.id))
        .bind(original)
        .bind(&db_path)
        .execute(&ctx.state.db)
        .await;
    match inserted {
        Ok(result) => Ok(Ok(result.last_insert_id() as u32)),
        Err(e) => {
            // Pas de fichier orphelin si la base refuse l'enregistrement.
            let _ = tokio::fs::remove_file(folder.join(&file_name)).await;
            Err(e.into())
        }
    }
}

/// Supprime un fichier (ligne `nf_file` et fichier sur le disque).
pub async fn delete(ctx: &Ctx, file_id: u32) -> Result<(), AppError> {
    let row = sqlx::query("SELECT path FROM nf_file WHERE id = ?")
        .bind(file_id)
        .fetch_optional(&ctx.state.db)
        .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let path: String = row.try_get("path")?;

    sqlx::query("DELETE FROM nf_file WHERE id = ?")
        .bind(file_id)
        .execute(&ctx.state.db)
        .await?;
    if let Some(disk) = disk_path(ctx, &path) {
        if let Err(e) = tokio::fs::remove_file(&disk).await {
            tracing::warn!(path = %disk.display(), error = %e, "fichier envoyé introuvable à la suppression");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(width, height, Rgba([10, 200, 150, 255]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn avatar_is_cropped_square() {
        let (bytes, ext) = process(&png(400, 120), Fit::Square(250)).unwrap();
        assert_eq!(ext, "png");
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (250, 250));
    }

    #[test]
    fn wide_images_are_reduced_and_small_ones_kept() {
        let (bytes, _) = process(&png(3000, 1500), Fit::MaxWidth(1600)).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (1600, 800));

        let (bytes, _) = process(&png(300, 200), Fit::MaxWidth(1600)).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (300, 200));
    }

    #[test]
    fn only_upload_paths_are_deleted() {
        assert_eq!(relative_upload_path("upload/news/a.jpg"), Some("news/a.jpg"));
        assert_eq!(relative_upload_path("./upload/news/a.jpg"), Some("news/a.jpg"));
        for bad in ["upload/", "upload/../neofrag.toml", "upload/a//b", "/etc/passwd", "images/logo.png", "upload/a/./b", "upload/..\\x"] {
            assert_eq!(relative_upload_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn non_images_are_rejected() {
        assert!(process(b"<?php echo 1; ?>", Fit::Square(250)).is_err());
        assert!(process(b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>", Fit::Square(250)).is_err());
        assert!(process(b"", Fit::Square(250)).is_err());
        // En-tête PNG valide, contenu tronqué.
        assert!(process(&png(50, 50)[..40], Fit::Square(250)).is_err());
    }
}
