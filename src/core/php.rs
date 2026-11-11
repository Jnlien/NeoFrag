//! Lecture / écriture du format `serialize()` de PHP.
//!
//! NeoFrag stocke plusieurs colonnes sous cette forme (`nf_dispositions.disposition`,
//! `nf_widgets.settings`, `nf_addon.data`…). Pour rester compatible avec une base
//! existante, on sait les relire et les réécrire à l'identique.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Tableau PHP : liste ordonnée de couples clé/valeur.
    Array(Vec<(Value, Value)>),
    /// Objet PHP : nom de classe + propriétés (noms nettoyés de leur préfixe de visibilité).
    Object(String, Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Str(s) => s.parse().ok(),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    /// Équivalent du test de vérité PHP (`!empty($x)`).
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty() && s != "0",
            Value::Array(a) => !a.is_empty(),
            Value::Object(..) => true,
        }
    }

    /// Accès `$array['key']` ou `$object->key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Array(items) => items.iter().find_map(|(k, v)| match k {
                Value::Str(s) if s == key => Some(v),
                Value::Int(i) if i.to_string() == key => Some(v),
                _ => None,
            }),
            Value::Object(_, props) => props.iter().find_map(|(k, v)| (k == key).then_some(v)),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// Valeurs d'un tableau, dans l'ordre.
    pub fn items(&self) -> Vec<&Value> {
        match self {
            Value::Array(items) => items.iter().map(|(_, v)| v).collect(),
            _ => Vec::new(),
        }
    }

    pub fn class(&self) -> Option<&str> {
        match self {
            Value::Object(class, _) => Some(class),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PHP unserialize: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

pub fn unserialize(input: &str) -> Result<Value, ParseError> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        pos: 0,
        depth: 0,
    };
    parser.value()
}

/// Profondeur d'imbrication maximale acceptée (protège la pile contre une donnée corrompue).
const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err<T>(&self, msg: &str) -> Result<T, ParseError> {
        Err(ParseError(format!("{msg} (position {})", self.pos)))
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.bytes.get(self.pos) == Some(&byte) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(&format!("'{}' attendu", byte as char))
        }
    }

    /// Lit jusqu'au délimiteur (exclu) et le consomme.
    fn until(&mut self, delim: u8) -> Result<&str, ParseError> {
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == delim {
                let slice = &self.bytes[start..self.pos];
                self.pos += 1;
                return std::str::from_utf8(slice).or_else(|_| self.err("UTF-8 invalide"));
            }
            self.pos += 1;
        }
        self.err("fin de données inattendue")
    }

    fn number<T: std::str::FromStr>(&mut self, delim: u8) -> Result<T, ParseError> {
        let text = self.until(delim)?.to_owned();
        text.parse().or_else(|_| self.err("nombre invalide"))
    }

    /// Chaîne de `len` octets entourée de guillemets.
    fn quoted(&mut self) -> Result<String, ParseError> {
        let len: usize = self.number(b':')?;
        self.expect(b'"')?;
        let end = self.pos.checked_add(len).filter(|&e| e <= self.bytes.len());
        let Some(end) = end else {
            return self.err("longueur de chaîne invalide");
        };
        let text = match std::str::from_utf8(&self.bytes[self.pos..end]) {
            Ok(text) => text.to_owned(),
            Err(_) => return self.err("UTF-8 invalide"),
        };
        self.pos = end;
        self.expect(b'"')?;
        Ok(text)
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        let Some(&tag) = self.bytes.get(self.pos) else {
            return self.err("fin de données inattendue");
        };
        self.pos += 1;

        match tag {
            b'N' => {
                self.expect(b';')?;
                Ok(Value::Null)
            }
            b'b' => {
                self.expect(b':')?;
                let n: i64 = self.number(b';')?;
                Ok(Value::Bool(n != 0))
            }
            b'i' => {
                self.expect(b':')?;
                Ok(Value::Int(self.number(b';')?))
            }
            b'd' => {
                self.expect(b':')?;
                Ok(Value::Float(self.number(b';')?))
            }
            b's' => {
                self.expect(b':')?;
                let text = self.quoted()?;
                self.expect(b';')?;
                Ok(Value::Str(text))
            }
            b'a' => {
                self.expect(b':')?;
                let count: usize = self.number(b':')?;
                Ok(Value::Array(self.pairs(count)?))
            }
            b'O' => {
                self.expect(b':')?;
                let class = self.quoted()?;
                self.expect(b':')?;
                let count: usize = self.number(b':')?;
                let props = self
                    .pairs(count)?
                    .into_iter()
                    .map(|(key, value)| {
                        let name = match key {
                            Value::Str(s) => s,
                            Value::Int(i) => i.to_string(),
                            _ => String::new(),
                        };
                        // "\0*\0nom" (protected) ou "\0Classe\0nom" (private)
                        let name = match name.rfind('\0') {
                            Some(i) => name[i + 1..].to_owned(),
                            None => name,
                        };
                        (name, value)
                    })
                    .collect();
                Ok(Value::Object(class, props))
            }
            _ => {
                self.pos -= 1;
                self.err("type inconnu")
            }
        }
    }

    fn pairs(&mut self, count: usize) -> Result<Vec<(Value, Value)>, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.err("imbrication trop profonde");
        }
        self.expect(b'{')?;
        // `count` vient de la donnée : on ne pré-alloue pas dessus.
        let mut pairs = Vec::new();
        for _ in 0..count {
            let key = self.value()?;
            let value = self.value()?;
            pairs.push((key, value));
        }
        self.expect(b'}')?;
        self.depth -= 1;
        Ok(pairs)
    }
}

/// Écriture au format PHP. Les propriétés d'objet reçoivent le préfixe `\0*\0` (protected)
/// attendu par NeoFrag. Servira à l'édition des widgets et des dispositions (ROADMAP).
#[allow(dead_code)]
pub fn serialize(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

#[allow(dead_code)]
fn write_str(out: &mut String, s: &str) {
    let _ = write!(out, "s:{}:\"{}\";", s.len(), s);
}

#[allow(dead_code)]
fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("N;"),
        Value::Bool(b) => {
            let _ = write!(out, "b:{};", *b as u8);
        }
        Value::Int(i) => {
            let _ = write!(out, "i:{i};");
        }
        Value::Float(f) => {
            let _ = write!(out, "d:{f};");
        }
        Value::Str(s) => write_str(out, s),
        Value::Array(items) => {
            let _ = write!(out, "a:{}:{{", items.len());
            for (k, v) in items {
                write_value(out, k);
                write_value(out, v);
            }
            out.push('}');
        }
        Value::Object(class, props) => {
            let _ = write!(out, "O:{}:\"{}\":{}:{{", class.len(), class, props.len());
            for (k, v) in props {
                write_str(out, &format!("\0*\0{k}"));
                write_value(out, v);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_and_arrays() {
        let v = unserialize(r##"a:2:{s:5:"title";s:8:"Facebook";s:3:"url";s:1:"#";}"##).unwrap();
        assert_eq!(v.get_str("title"), Some("Facebook"));
        assert_eq!(v.get_str("url"), Some("#"));

        let v = unserialize("a:1:{s:7:\"enabled\";b:1;}").unwrap();
        assert!(v.get("enabled").unwrap().truthy());
    }

    #[test]
    fn multibyte_lengths_are_bytes() {
        let v = unserialize("s:7:\"Équipe\";").unwrap();
        assert_eq!(v.as_str(), Some("Équipe"));
    }

    #[test]
    fn neofrag_disposition() {
        let raw = "O:27:\"NF\\NeoFrag\\Libraries\\Array_\":1:{s:9:\"\0*\0_array\";a:1:{i:0;O:27:\"NF\\NeoFrag\\Displayables\\Row\":2:{s:9:\"\0*\0_array\";a:1:{i:0;O:27:\"NF\\NeoFrag\\Displayables\\Col\":2:{s:9:\"\0*\0_array\";a:1:{i:0;O:30:\"NF\\NeoFrag\\Displayables\\Widget\":3:{s:10:\"\0*\0_widget\";i:4;s:9:\"\0*\0_style\";N;s:8:\"\0*\0_size\";N;}}s:8:\"\0*\0_size\";N;}}s:9:\"\0*\0_style\";s:18:\"align-items-center\";}}}";
        let v = unserialize(raw).unwrap();
        let row = v.get("_array").unwrap().items()[0];
        assert_eq!(row.class(), Some("NF\\NeoFrag\\Displayables\\Row"));
        assert_eq!(row.get_str("_style"), Some("align-items-center"));
        let widget = row.get("_array").unwrap().items()[0].get("_array").unwrap().items()[0];
        assert_eq!(widget.get("_widget").unwrap().as_i64(), Some(4));

        // Aller-retour à l'identique : PHP doit pouvoir relire ce qu'on écrit.
        assert_eq!(serialize(&v), raw);
    }

    #[test]
    fn rejects_garbage() {
        assert!(unserialize("").is_err());
        assert!(unserialize("s:99:\"court\";").is_err());
        assert!(unserialize("a:1:{i:0;").is_err());
        assert!(unserialize("x:1;").is_err());
    }
}
