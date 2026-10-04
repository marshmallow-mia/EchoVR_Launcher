//! Steam's two KeyValues formats: the binary one of `shortcuts.vdf` (non-Steam games), read
//! and written, and the text one of `loginusers.vdf` (who signed in last), read. Keys keep
//! their file order, so a file read and written back is unchanged apart from what changed.

use anyhow::{bail, Context, Result};

// ---- binary (shortcuts.vdf) ----

/// A binary KeyValues value.
#[derive(Debug, Clone, PartialEq)]
pub enum Bin {
    Map(Vec<(String, Bin)>),
    Str(String),
    Int(u32),
    Float(f32),
    U64(u64),
    I64(i64),
}

const MAP: u8 = 0x00;
const STR: u8 = 0x01;
const INT: u8 = 0x02;
const FLOAT: u8 = 0x03;
const U64: u8 = 0x07;
const END: u8 = 0x08;
const I64: u8 = 0x0A;

impl Bin {
    pub fn get(&self, key: &str) -> Option<&Bin> {
        match self {
            Bin::Map(m) => m
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Bin::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.bytes.get(self.at).context("the file ends early")?;
        self.at += 1;
        Ok(b)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self
            .bytes
            .get(self.at..self.at + N)
            .context("the file ends early")?;
        self.at += N;
        Ok(s.try_into().expect("N bytes"))
    }

    fn cstr(&mut self) -> Result<String> {
        let rest = &self.bytes[self.at..];
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .context("a string has no end")?;
        self.at += end + 1;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    /// Pairs until a map end (or the end of the file at the top level).
    fn map(&mut self, top: bool) -> Result<Vec<(String, Bin)>> {
        let mut out = Vec::new();
        loop {
            if top && self.at == self.bytes.len() {
                return Ok(out);
            }
            let kind = self.byte()?;
            if kind == END {
                return Ok(out);
            }
            let key = self.cstr()?;
            let value = match kind {
                MAP => Bin::Map(self.map(false)?),
                STR => Bin::Str(self.cstr()?),
                INT => Bin::Int(u32::from_le_bytes(self.take()?)),
                FLOAT => Bin::Float(f32::from_le_bytes(self.take()?)),
                U64 => Bin::U64(u64::from_le_bytes(self.take()?)),
                I64 => Bin::I64(i64::from_le_bytes(self.take()?)),
                other => bail!("unknown value type {other:#04x} for {key:?}"),
            };
            out.push((key, value));
        }
    }
}

/// Reads a binary KeyValues file: its top-level pairs.
pub fn parse_bin(bytes: &[u8]) -> Result<Vec<(String, Bin)>> {
    Reader { bytes, at: 0 }.map(true)
}

fn write_map(out: &mut Vec<u8>, pairs: &[(String, Bin)]) {
    for (k, v) in pairs {
        let key = |out: &mut Vec<u8>, kind: u8| {
            out.push(kind);
            out.extend_from_slice(k.as_bytes());
            out.push(0);
        };
        match v {
            Bin::Map(m) => {
                key(out, MAP);
                write_map(out, m);
                out.push(END);
            }
            Bin::Str(s) => {
                key(out, STR);
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Bin::Int(i) => {
                key(out, INT);
                out.extend_from_slice(&i.to_le_bytes());
            }
            Bin::Float(f) => {
                key(out, FLOAT);
                out.extend_from_slice(&f.to_le_bytes());
            }
            Bin::U64(u) => {
                key(out, U64);
                out.extend_from_slice(&u.to_le_bytes());
            }
            Bin::I64(i) => {
                key(out, I64);
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
    }
}

/// Writes top-level pairs as Steam does: each map closed, then the file closed.
pub fn write_bin(pairs: &[(String, Bin)]) -> Vec<u8> {
    let mut out = Vec::new();
    write_map(&mut out, pairs);
    out.push(END);
    out
}

// ---- text (config.vdf) ----

/// A text KeyValues value.
#[derive(Debug, Clone, PartialEq)]
pub enum Text {
    Str(String),
    Map(Vec<(String, Text)>),
}

impl Text {
    pub fn get(&self, key: &str) -> Option<&Text> {
        match self {
            Text::Map(m) => m
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Text::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Text::Str(s) => Some(s),
            Text::Map(_) => None,
        }
    }
}

/// Reads a text KeyValues file into one map of its top-level pairs.
pub fn parse_text(text: &str) -> Result<Text> {
    let tokens = tokenize(text)?;
    let mut it = tokens.into_iter().peekable();
    let top = text_map(&mut it, true)?;
    Ok(Text::Map(top))
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match chars.next().context("a quoted string has no end")? {
                        '"' => break,
                        '\\' => match chars.next().context("a quoted string has no end")? {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            other => s.push(other),
                        },
                        other => s.push(other),
                    }
                }
                out.push(Tok::Str(s));
            }
            other => {
                // An unquoted word.
                let mut s = String::from(other);
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                out.push(Tok::Str(s));
            }
        }
    }
    Ok(out)
}

fn text_map(
    it: &mut std::iter::Peekable<std::vec::IntoIter<Tok>>,
    top: bool,
) -> Result<Vec<(String, Text)>> {
    let mut out = Vec::new();
    loop {
        match it.next() {
            None if top => return Ok(out),
            None => bail!("a block has no end"),
            Some(Tok::Close) if !top => return Ok(out),
            Some(Tok::Close) | Some(Tok::Open) => bail!("unexpected brace"),
            Some(Tok::Str(key)) => match it.next() {
                Some(Tok::Str(v)) => out.push((key, Text::Str(v))),
                Some(Tok::Open) => out.push((key, Text::Map(text_map(it, false)?))),
                _ => bail!("{key:?} has no value"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shortcuts.vdf as Steam writes it, with one non-Steam game.
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"\x00shortcuts\x00");
        b.extend_from_slice(b"\x000\x00");
        b.extend_from_slice(b"\x02appid\x00");
        b.extend_from_slice(&0x8123_4567u32.to_le_bytes());
        b.extend_from_slice(b"\x01AppName\x00Some Game\x00");
        b.extend_from_slice(b"\x01Exe\x00\"/usr/bin/game\"\x00");
        b.extend_from_slice(b"\x02IsHidden\x00\x00\x00\x00\x00");
        b.extend_from_slice(b"\x00tags\x00\x08");
        b.extend_from_slice(b"\x08\x08\x08");
        b
    }

    #[test]
    fn binary_round_trip() {
        let pairs = parse_bin(&sample()).unwrap();
        assert_eq!(pairs.len(), 1);
        let shortcuts = &pairs[0].1;
        let game = shortcuts.get("0").unwrap();
        assert_eq!(game.get("appid"), Some(&Bin::Int(0x8123_4567)));
        assert_eq!(game.get("appname").and_then(Bin::as_str), Some("Some Game"));
        assert_eq!(write_bin(&pairs), sample());
    }

    #[test]
    fn binary_errors() {
        assert!(parse_bin(b"\x01key\x00no end").is_err());
        assert!(parse_bin(b"\x09key\x00").is_err());
        assert!(parse_bin(b"").unwrap().is_empty());
    }

    const CONFIG: &str = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"0"
					{
						"name"		"proton_9"
						"config"		""
						"priority"		"75"
					}
				}
				"Path \"quoted\""		"C:\\Steam"
			}
		}
	}
}
"#;

    #[test]
    fn reads_text() {
        let t = parse_text(CONFIG).unwrap();
        let steam = t
            .get("InstallConfigStore")
            .and_then(|s| s.get("Software"))
            .and_then(|s| s.get("valve"))
            .and_then(|s| s.get("Steam"))
            .unwrap();
        assert_eq!(
            steam
                .get("CompatToolMapping")
                .and_then(|m| m.get("0"))
                .and_then(|g| g.get("name"))
                .and_then(Text::as_str),
            Some("proton_9")
        );
        assert_eq!(
            steam.get("Path \"quoted\"").and_then(Text::as_str),
            Some("C:\\Steam")
        );
    }

    #[test]
    fn text_errors() {
        assert!(parse_text("\"a\" {").is_err());
        assert!(parse_text("\"a\"").is_err());
    }
}
