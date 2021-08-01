use std::fs::File;
use std::io::{self, BufRead, BufReader};

use cgi;
use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::Serialize;

cgi::cgi_main! { |request: cgi::Request| -> cgi::Response {
    if let Some(query) = request.uri().query() {
        let parsed_query = parse_query(query);
        let result = match get_result(&parsed_query) {
            Ok(words) => ResponseResult {
                success: true,
                message: None,
                words,
            },
            Err(_) => ResponseResult {
                success: false,
                message: Some(String::from("Error fetching result.")),
                words: Vec::new(),
            },
        };
        match serde_json::to_string(&result) {
            Err(_) => cgi::text_response(403, "Internal Error: Error serializing JSON"),
            Ok(r) => cgi::text_response(200, &r)
        }
    } else {
        cgi::text_response(400, "word lookup requires query parameters")
    }
} }

#[derive(Serialize, Debug)]
struct ResponseResult {
    success: bool,
    message: Option<String>,
    words: Vec<Word>,
}

#[derive(Debug)]
struct Query {
    orthography: String,
    transcription: String,
    num_syllables: Option<i32>,
    stress: Option<String>,
    morph_orthography: Option<String>,
    morph_transcription: Option<String>,
    num_morphemes: Option<i32>,
    min_frequency: Option<i32>,
    max_frequency: Option<i32>,
    part_of_speech: Option<String>,
}

#[derive(Serialize, Debug)]
struct Word {
    orthography: String,
    transcription: String,
    num_syllables: i32,
    stress: String,
    morph_orthography: String,
    morph_transcription: String,
    num_morphemes: i32,
    part_of_speech: String,
    hal_frequency: i32,
    hal_frequency_log: f32,
}

fn parse_query(query: &str) -> Query {
    let mut result = Query {
        orthography: String::from(""),
        transcription: String::from(""),
        num_syllables: None,
        stress: None,
        morph_orthography: None,
        morph_transcription: None,
        num_morphemes: None,
        min_frequency: None,
        max_frequency: None,
        part_of_speech: None,
    };
    for parameter in query.split("&") {
        if let Some((key, value)) = parameter.split_once('=') {
            if let Ok(value) = percent_decode_str(value).decode_utf8() {
                let value = value.to_string();
                match key {
                    "orthography" => result.orthography = value,
                    "transcription" => result.transcription = value,
                    "num_syllables" => result.num_syllables = value.parse().ok(),
                    "stress" => result.stress = string_or_none(value),
                    "morph_orthography" => result.morph_orthography = string_or_none(value),
                    "morph_transcription" => result.morph_transcription = string_or_none(value),
                    "num_morphemes" => result.num_morphemes = value.parse().ok(),
                    "min_frequency" => result.min_frequency = value.parse().ok(),
                    "max_frequency" => result.max_frequency = value.parse().ok(),
                    "part_of_speech" => result.part_of_speech = string_or_none(value),
                    _ => (),
                }
            }
        }
    }
    result
}

fn clean_regex(re: &str) -> Regex {
    let re = re
        .replace(r"\", r"\\")
        .replace('[', r"\[")
        .replace('{', r"\{")
        .replace(r"(?", r"(\?");
    Regex::new(re.as_str()).unwrap()
}

fn get_result(query: &Query) -> io::Result<Vec<Word>> {
    let dictionary = BufReader::new(File::open("elpdic")?);
    let mut result = Vec::new();
    let orthography_regex = clean_regex(&query.orthography);
    let transcription_regex = clean_regex(&query.transcription);
    let stress_regex = match &query.stress {
        Some(stress) => Some(clean_regex(stress)),
        None => None,
    };
    let pos_regex = match &query.part_of_speech {
        Some(pos) => Some(clean_regex(pos)),
        None => None,
    };
    let morph_orthography_regex = match &query.morph_orthography {
        Some(morph_orthography) => Some(clean_regex(morph_orthography)),
        None => None,
    };
    let morph_transcription_regex = match &query.morph_transcription {
        Some(morph_transcription) => Some(clean_regex(morph_transcription)),
        None => None,
    };
    for line in dictionary.lines() {
        let line = line.unwrap();
        let columns: Vec<&str> = line.split('\t').collect();
        let word = Word {
            orthography: String::from(columns[0]),
            transcription: String::from(columns[1]),
            num_syllables: columns[2].parse().unwrap_or(0),
            stress: String::from(columns[3]),
            morph_orthography: String::from(columns[4]),
            morph_transcription: String::from(columns[5]),
            num_morphemes: columns[6].parse().unwrap_or(0),
            part_of_speech: String::from(columns[7]),
            hal_frequency: columns[8].parse().unwrap_or(0),
            hal_frequency_log: columns[9].parse().unwrap_or(0f32),
        };
        if !(orthography_regex.is_match(word.orthography.as_str())
            && transcription_regex.is_match(word.transcription.as_str()))
        {
            continue;
        }
        if let Some(num_syllables) = query.num_syllables {
            if word.num_syllables != num_syllables {
                continue;
            };
        }
        if let Some(stress_regex) = &stress_regex {
            if !stress_regex.is_match(word.stress.as_str()) {
                continue;
            }
        }
        if let Some(morph_orthography_regex) = &morph_orthography_regex {
            if !morph_orthography_regex.is_match(word.morph_orthography.as_str()) {
                continue;
            }
        }
        if let Some(morph_transcription_regex) = &morph_transcription_regex {
            if !morph_transcription_regex.is_match(word.morph_transcription.as_str()) {
                continue;
            }
        }
        if let Some(num_morphemes) = query.num_morphemes {
            if word.num_morphemes != num_morphemes {
                continue;
            };
        }
        if let Some(min_frequency) = query.min_frequency {
            if word.hal_frequency < min_frequency {
                continue;
            };
        }
        if let Some(max_frequency) = query.max_frequency {
            if word.hal_frequency > max_frequency {
                continue;
            };
        }
        if let Some(pos_regex) = &pos_regex {
            if !pos_regex.is_match(word.part_of_speech.as_str()) {
                continue;
            };
        }
        result.push(word);
    }
    Ok(result)
}

fn string_or_none(string: String) -> Option<String> {
    if string.len() > 0 {
        return Some(string);
    }
    None
}
