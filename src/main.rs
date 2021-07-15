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
    min_frequency: Option<i32>,
    max_frequency: Option<i32>,
    part_of_speech: Option<String>,
}

#[derive(Serialize, Debug)]
struct Word {
    spelling: String,
    transcription: String,
    num_syllables: i32,
    stress: String,
    spelling_morph: String,
    transcription_morph: String,
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
                    "syllables" => {
                        if let Ok(num_syllables) = value.parse() {
                            result.num_syllables = Some(num_syllables);
                        }
                    }
                    "stress" => result.stress = string_or_none(value),
                    "minfrequency" => {
                        if let Ok(min_frequency) = value.parse() {
                            result.min_frequency = Some(min_frequency);
                        }
                    }
                    "maxfrequency" => {
                        if let Ok(max_frequency) = value.parse() {
                            result.max_frequency = Some(max_frequency);
                        }
                    }
                    "pos" => result.part_of_speech = string_or_none(value),
                    _ => (),
                }
            }
        }
    }
    result
}

fn get_result(query: &Query) -> io::Result<Vec<Word>> {
    let dictionary = BufReader::new(File::open("elpdic")?);
    let mut result = Vec::new();
    let orthography_regex = Regex::new(&query.orthography).unwrap();
    let transcription_regex = Regex::new(&query.transcription).unwrap();
    let mut stress_regex = None;
    if let Some(stress) = &query.stress {
        stress_regex = Some(Regex::new(stress).unwrap());
    }
    let mut pos_regex = None;
    if let Some(pos) = &query.part_of_speech {
        pos_regex = Some(Regex::new(pos).unwrap());
    }
    for line in dictionary.lines() {
        let line = line.unwrap();
        let columns: Vec<&str> = line.split('\t').collect();
        let word = Word {
            spelling: String::from(columns[0]),
            transcription: String::from(columns[1]),
            num_syllables: columns[2].parse().unwrap_or(0),
            stress: String::from(columns[3]),
            spelling_morph: String::from(columns[4]),
            transcription_morph: String::from(columns[5]),
            num_morphemes: columns[6].parse().unwrap_or(0),
            part_of_speech: String::from(columns[7]),
            hal_frequency: columns[8].parse().unwrap_or(0),
            hal_frequency_log: columns[9].parse().unwrap_or(0f32),
        };
        if !(orthography_regex.is_match(word.spelling.as_str())
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
