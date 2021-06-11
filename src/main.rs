use chrono::prelude::*;
use std::env;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::prelude::*;
use std::io::{BufReader, SeekFrom};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::str;

use httparse;
use percent_encoding::percent_decode_str;
use rusqlite::{params_from_iter, Connection, LoadExtensionGuard, ToSql, Transaction};
use serde::Serialize;

enum HTTPError {
    BadRequest,
    NotFound,
    InternalError,
    Forbidden,
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

#[derive(Serialize, Debug)]
struct ResponseResult {
    success: bool,
    message: Option<String>,
    words: Vec<Word>,
}

struct Query {
    orthography: String,
    transcription: String,
    num_syllables: Option<i32>,
    stress: Option<String>,
    min_frequency: Option<i32>,
    max_frequency: Option<i32>,
    part_of_speech: Option<String>,
}

struct RequestData<'a> {
    stream: &'a TcpStream,
    conn: &'a mut Connection,
    buffer: [u8; 1024],
    total_read: usize,
    length: usize,
}

impl<'a> RequestData<'a> {
    fn new(stream: &'a mut TcpStream, conn: &'a mut Connection) -> RequestData<'a> {
        RequestData {
            stream,
            conn,
            buffer: [0; 1024],
            total_read: 0,
            length: 0,
        }
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    let args: Vec<String> = env::args().collect();
    let mut conn = Connection::open(&args[1]).unwrap();

    {
        let _guard = LoadExtensionGuard::new(&conn).unwrap();
        conn.load_extension(Path::new("/usr/lib/sqlite3/pcre.so"), None)
            .unwrap();
    }

    for stream in listener.incoming() {
        let mut now = Local::now();
        match stream {
            Ok(mut stream) => {
                let response = handle_connection(&mut stream, &mut conn);
                let response = match response {
                    Err(error) => match error {
                        HTTPError::NotFound => not_found(),
                        HTTPError::BadRequest => bad_request(),
                        HTTPError::InternalError => internal_error(),
                        HTTPError::Forbidden => forbidden(),
                    },
                    Ok(response) => response,
                };
                now = Local::now();
                match stream.write(response.as_bytes()) {
                    Ok(bytes_written) => println!(
                        "[{}] Wrote {} bytes back to client.",
                        now.to_rfc2822(),
                        bytes_written
                    ),
                    Err(e) => println!("[{}], {}", now, e),
                };
                if let Err(e) = stream.flush() {
                    now = Local::now();
                    println!("[{}], {}", now, e);
                }
            }
            Err(e) => {
                println!("[{}] {}", now, e);
            }
        };
    }
}

fn handle_connection(stream: &mut TcpStream, conn: &mut Connection) -> Result<String, HTTPError> {
    let mut response = None;
    let mut result = Err(HTTPError::BadRequest);
    let mut request = RequestData::new(stream, conn);

    if let Ok(bytes_read) = request.stream.read(&mut request.buffer) {
        request.total_read += bytes_read;
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut req = httparse::Request::new(&mut headers);
        let bytes_string = bytes_read.to_string();
        request.length = str::from_utf8(
            req.headers
                .iter()
                .find(|h| h.name == "Content-Length")
                .unwrap_or(&httparse::Header {
                    name: "Content-Length",
                    value: bytes_string.to_string().as_bytes(),
                })
                .value,
        )
        .unwrap_or_else(|_| bytes_string.as_str())
        .parse()
        .unwrap();

        if let Ok(_) = req.parse(&request.buffer) {
            let path = req.path.unwrap_or("/");
            let (path, query) = match path.split_once('?') {
                None => (path, ""),
                Some(path) => path,
            };
            let method = req.method.unwrap_or("GET");
            if method == "GET" {
                if path == "/" {
                    if let Ok(contents) = fs::read_to_string("html/index.html") {
                        result = Ok(build_response(&"200 OK", &contents));
                    } else {
                        return Err(HTTPError::NotFound);
                    }
                } else if path == "/search" {
                    let parsed_query = parse_query(query);
                    let result = match get_result(conn, &parsed_query) {
                        Ok(words) => ResponseResult {
                            success: true,
                            message: None,
                            words,
                        },
                        Err(_) => ResponseResult {
                            success: false,
                            message: Some(String::from("Error fetching result from database.")),
                            words: Vec::new(),
                        },
                    };
                    response = Some(result);
                }
            } else if method == "POST" && path == "/update" {
                println!("{:?}", query);
                if let Some(mut password_i) = query.find("password=") {
                    // index is the index of the password plus the length of "password="
                    password_i += 9;
                    let end_i = query[password_i..].find('&').unwrap_or(query.len());
                    if &query[password_i..end_i] == "Crossroads!" {
                        if let Ok(result) = update_database(&mut request) {
                            response = Some(result);
                        } else {
                            result = Err(HTTPError::InternalError);
                        }
                    } else {
                        result = Err(HTTPError::Forbidden);
                    }
                }
            }
        }
    }

    if let Some(object) = response {
        result = match serde_json::to_string(&object) {
            Err(_) => Err(HTTPError::InternalError),
            Ok(r) => Ok(build_response(&"200 OK", &r)),
        }
    }

    result
}

fn update_database(
    request: &mut RequestData,
) -> Result<ResponseResult, Box<dyn std::error::Error>> {
    let mut common_options = OpenOptions::new();
    common_options
        .read(true)
        .write(true)
        .create(true)
        .truncate(true);
    let mut file = &common_options.open("upload.txt")?;

    let mut end = request.buffer.len();
    file.write(&request.buffer[request.total_read..end])?;
    while request.total_read < request.length {
        let bytes_read = request.stream.read(&mut request.buffer)?;
        request.total_read += bytes_read;
        end = bytes_read;
        file.write(&request.buffer[..end])?;
    }

    file.seek(SeekFrom::Start(0))?;
    let file_reader = BufReader::new(file);

    let transaction = request.conn.transaction()?;

    insert_lines(file_reader, &transaction)?;

    transaction.commit()?;

    fs::remove_file("upload.txt")?;
    Ok(ResponseResult {
        success: true,
        message: Some(String::from("Successfully updated database.")),
        words: Vec::new(),
    })
}

fn insert_lines(
    file_reader: BufReader<&File>,
    transaction: &Transaction,
) -> Result<(), Box<dyn std::error::Error>> {
    transaction.execute("DELETE FROM english_words", [])?;

    let mut statement = transaction.prepare("INSERT INTO english_words VALUES (?, ?, CAST(? AS INTEGER), ?, ?, ?, CAST(? AS INTEGER), ?, CAST(? AS INTEGER), CAST(? AS REAL))")?;

    for line in file_reader.lines() {
        let line = line.unwrap_or_else(|_| String::from("")).replace('ǝ', "ə");
        let fields = line.split('\t');
        statement.execute(params_from_iter(fields))?;
    }

    Ok(())
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

fn get_result(conn: &Connection, query: &Query) -> Result<Vec<Word>, rusqlite::Error> {
    let mut sql = String::from(
        r#"
        SELECT 
            spelling,
            transcription,
            num_syllables,
            stress,
            spelling_morph,
            transcription_morph,
            num_morphemes,
            part_of_speech,
            hal_frequency,
            hal_frequency_log
        FROM english_words
        WHERE
            spelling REGEXP ? AND
            transcription REGEXP ?
        "#,
    );
    let mut parameters: Vec<&dyn ToSql> = vec![&query.orthography, &query.transcription];
    if let Some(stress) = &query.stress {
        sql.push_str(" AND stress REGEXP ?");
        parameters.push(stress);
    }
    if let Some(part_of_speech) = &query.part_of_speech {
        parameters.push(part_of_speech);
    }
    if let Some(num_syllables) = &query.num_syllables {
        sql.push_str(" AND num_syllables = ?");
        parameters.push(num_syllables);
    }
    if let Some(min_frequency) = &query.min_frequency {
        sql.push_str(" AND hal_frequency >= ?");
        parameters.push(min_frequency);
    }
    if let Some(max_frequency) = &query.max_frequency {
        sql.push_str(" AND hal_frequency <= ?");
        parameters.push(max_frequency);
    }

    let mut statement = conn.prepare(&sql)?;
    let words: Vec<Word> = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok(Word {
                spelling: row.get(0)?,
                transcription: row.get(1)?,
                num_syllables: row.get(2)?,
                stress: row.get(3)?,
                spelling_morph: row.get(4)?,
                transcription_morph: row.get(5)?,
                num_morphemes: row.get(6)?,
                part_of_speech: row.get(7)?,
                hal_frequency: row.get(8)?,
                hal_frequency_log: row.get(9)?,
            })
        })?
        .map(|word| word.unwrap())
        .collect();

    Ok(words)
}

fn string_or_none(string: String) -> Option<String> {
    if string.len() > 0 {
        return Some(string);
    }
    None
}

fn bad_request() -> String {
    let contents = "400 Bad Request";
    build_response(&contents, &contents)
}

fn not_found() -> String {
    let contents = "404 Not Found";
    build_response(&contents, &contents)
}

fn internal_error() -> String {
    let contents = "500 Internal Server Error";
    build_response(&contents, &contents)
}

fn forbidden() -> String {
    let contents = "403 Forbidden";
    build_response(&contents, &contents)
}

fn build_response(status: &str, contents: &str) -> String {
    format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\n\r\n{}",
        status,
        contents.len(),
        contents
    )
}
