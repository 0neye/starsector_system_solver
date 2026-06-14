//! Save discovery + descriptor.xml parsing. See workspace/SAVE_EXTRACTION_DESIGN.md.
//! PUBLIC SIGNATURES ARE PINNED - bin/extract.rs is written against them.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::extract::model::SaveInfo;
use crate::extract::{ExtractError, Result};

impl From<csv::Error> for ExtractError {
    fn from(e: csv::Error) -> Self {
        ExtractError::BadSave(e.to_string())
    }
}

pub fn discover_saves(saves_dir: &Path) -> Result<Vec<SaveInfo>> {
    let mut saves = Vec::new();

    for entry in fs::read_dir(saves_dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("warning: failed to read save directory entry: {err}");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().into_owned();
        if !dir_name.starts_with("save_") {
            continue;
        }

        let descriptor_path = path.join("descriptor.xml");
        if !descriptor_path.exists() {
            continue;
        }

        let campaign_path = path.join("campaign.xml");
        let modified = campaign_path
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH);

        let descriptor = match fs::read_to_string(&descriptor_path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!(
                    "warning: skipping save {dir_name} because descriptor.xml could not be read: {err}"
                );
                continue;
            }
        };

        let parsed = parse_descriptor(&descriptor);
        saves.push(SaveInfo {
            dir_name,
            path,
            character_name: parsed.character_name,
            save_date: parsed.save_date,
            game_version: parsed.game_version,
            character_level: parsed.character_level,
            compressed: parsed.compressed,
            modified,
        });
    }

    saves.sort_by(|a, b| {
        let a_key = save_sort_key(a);
        let b_key = save_sort_key(b);
        b_key.cmp(&a_key)
    });

    Ok(saves)
}

pub fn load_campaign_xml(save: &SaveInfo) -> Result<String> {
    let path = save.path.join("campaign.xml");
    let mut file = fs::File::open(&path)?;

    if save.compressed {
        let mut decoder = GzDecoder::new(file);
        let mut xml = String::new();
        decoder.read_to_string(&mut xml).map_err(|err| {
            ExtractError::BadSave(format!("failed to decompress {}: {err}", path.display()))
        })?;
        return Ok(xml);
    }

    let mut xml = String::new();
    file.read_to_string(&mut xml)?;
    Ok(xml)
}

struct DescriptorData {
    character_name: String,
    save_date: String,
    game_version: String,
    character_level: u32,
    compressed: bool,
    compressed_seen: bool,
}

fn parse_descriptor(xml: &str) -> DescriptorData {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut current_field: Option<String> = None;
    let mut data = DescriptorData {
        character_name: String::new(),
        save_date: String::new(),
        game_version: String::new(),
        character_level: 0,
        compressed: false,
        compressed_seen: false,
    };

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                // Only the first occurrence of each field counts (mod specs nest
                // their own gameVersion etc. deeper in the document), and any
                // nested element ends the field we were capturing.
                if is_descriptor_field(&tag) && !field_filled(&data, &tag) {
                    current_field = Some(tag);
                } else {
                    current_field = None;
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(field) = current_field.as_deref() {
                    let text = String::from_utf8_lossy(event.as_ref()).into_owned();
                    assign_field(&mut data, field, text);
                }
            }
            Ok(Event::CData(event)) => {
                if let Some(field) = current_field.as_deref() {
                    let text = String::from_utf8_lossy(event.as_ref()).into_owned();
                    assign_field(&mut data, field, text);
                }
            }
            Ok(Event::End(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if current_field.as_deref() == Some(tag.as_str()) {
                    current_field = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    data
}

fn is_descriptor_field(tag: &str) -> bool {
    matches!(
        tag,
        "characterName" | "saveDate" | "gameVersion" | "characterLevel" | "compressed"
    )
}

fn field_filled(data: &DescriptorData, tag: &str) -> bool {
    match tag {
        "characterName" => !data.character_name.is_empty(),
        "saveDate" => !data.save_date.is_empty(),
        "gameVersion" => !data.game_version.is_empty(),
        "characterLevel" => data.character_level != 0,
        "compressed" => data.compressed_seen,
        _ => true,
    }
}

fn assign_field(data: &mut DescriptorData, field: &str, text: String) {
    match field {
        "characterName" => data.character_name = text,
        "saveDate" => data.save_date = text,
        "gameVersion" => data.game_version = text,
        "characterLevel" => data.character_level = text.parse::<u32>().unwrap_or(0),
        "compressed" => {
            data.compressed = text.eq_ignore_ascii_case("true");
            data.compressed_seen = true;
        }
        _ => {}
    }
}

fn save_sort_key(save: &SaveInfo) -> i128 {
    if let Some(ts) = parse_utc_timestamp(&save.save_date) {
        return ts;
    }

    match save.modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(_) => 0,
    }
}

fn parse_utc_timestamp(raw: &str) -> Option<i128> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_suffix(" UTC").unwrap_or(trimmed);
    let (date_part, time_part) = trimmed.split_once(' ')?;

    let mut date_iter = date_part.split('-');
    let year = date_iter.next()?.parse::<i32>().ok()?;
    let month = date_iter.next()?.parse::<u32>().ok()?;
    let day = date_iter.next()?.parse::<u32>().ok()?;

    let mut time_iter = time_part.split(':');
    let hour = time_iter.next()?.parse::<u32>().ok()?;
    let minute = time_iter.next()?.parse::<u32>().ok()?;
    let second_part = time_iter.next()?;
    let second = second_part.split('.').next()?.parse::<u32>().ok()?;

    let days = days_from_civil(year, month, day)?;
    let seconds =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    Some(i128::from(seconds) * 1_000_000_000)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }

    let month_lengths = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut max_day = month_lengths[(month - 1) as usize];
    if month == 2 && is_leap_year(year) {
        max_day = 29;
    }
    if day > max_day {
        return None;
    }

    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn parses_descriptor_and_sort_key() {
        let xml = r#"
<SaveGameData>
  <characterName>DEMIURGE</characterName>
  <saveDate>2026-05-19 06:06:55.231 UTC</saveDate>
  <gameVersion>0.98a-RC8</gameVersion>
  <characterLevel>6</characterLevel>
  <compressed>false</compressed>
</SaveGameData>
"#;
        let parsed = parse_descriptor(xml);
        assert_eq!(parsed.character_name, "DEMIURGE");
        assert_eq!(parsed.save_date, "2026-05-19 06:06:55.231 UTC");
        assert_eq!(parsed.game_version, "0.98a-RC8");
        assert_eq!(parsed.character_level, 6);
        assert!(!parsed.compressed);
        assert!(parse_utc_timestamp(&parsed.save_date).is_some());
    }

    #[test]
    fn load_campaign_xml_reads_plain_text() {
        let temp_dir = std::env::temp_dir().join("system_solver_extract_save_test");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        fs::write(temp_dir.join("campaign.xml"), "<root/>").unwrap();

        let save = SaveInfo {
            dir_name: "save_test".to_string(),
            path: PathBuf::from(&temp_dir),
            character_name: String::new(),
            save_date: String::new(),
            game_version: String::new(),
            character_level: 0,
            compressed: false,
            modified: UNIX_EPOCH + Duration::from_secs(1),
        };

        let xml = load_campaign_xml(&save).unwrap();
        assert_eq!(xml, "<root/>");
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
