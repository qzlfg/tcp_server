use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, mpsc::channel};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

#[derive(Debug)]
pub struct HttpRequestHead {
    pub method: HttpMethod,
    pub path: String,
    pub version: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug)]
pub struct UploadedFile {
    pub field_name: String,
    pub original_name: String,
    pub disk_path: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug)]
pub enum HttpParseError {
    /// Ошибка чтения из сокета (сеть оборвалась, таймаут и т.д.)
    Io(io::Error),
    /// Клиент отключился до отправки данных
    ConnectionClosed,
    /// Первая строка не состоит из 3 частей (METHOD PATH VERSION)
    InvalidRequestLine,
    /// Метод не распознан
    InvalidMethod(String),
    /// Строка заголовка не содержит символа ':'
    InvalidHeader,
    /// Заголовок Content-Length не является положительным числом
    InvalidContentLength,
    MultipartError(String),
}

#[derive(Debug)]
pub enum ConnectionError {
    /// I/O вне парсинга (например, ошибка записи ответа).
    Io(io::Error),
    /// Ошибка при разборе HTTP-запроса.
    Http(HttpParseError),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Io(e) => write!(f, "io: {e}"),
            ConnectionError::Http(e) => write!(f, "http: {e:?}"),
        }
    }
}

impl From<io::Error> for HttpParseError {
    fn from(err: io::Error) -> Self {
        HttpParseError::Io(err)
    }
}

impl FromStr for HttpMethod {
    type Err = HttpParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "GET" => Ok(HttpMethod::Get),
            "POST" => Ok(HttpMethod::Post),
            "PUT" => Ok(HttpMethod::Put),
            "DELETE" => Ok(HttpMethod::Delete),
            "PATCH" => Ok(HttpMethod::Patch),
            other => Err(HttpParseError::InvalidMethod(other.to_string())),
        }
    }
}

impl From<io::Error> for ConnectionError {
    fn from(err: io::Error) -> Self {
        ConnectionError::Io(err)
    }
}

impl HttpRequestHead {
    pub fn from_reader(reader: &mut BufReader<TcpStream>) -> Result<Self, HttpParseError> {
        let mut line_buf = String::new();

        let bytes_read = reader.read_line(&mut line_buf)?;
        if bytes_read == 0 {
            return Err(HttpParseError::ConnectionClosed);
        }

        let mut parts = line_buf.split_whitespace();

        let method_raw = parts.next().ok_or(HttpParseError::InvalidRequestLine)?;
        let path = parts
            .next()
            .ok_or(HttpParseError::InvalidRequestLine)?
            .to_string();
        let version = parts
            .next()
            .ok_or(HttpParseError::InvalidRequestLine)?
            .to_string();
        let method: HttpMethod = method_raw.parse()?;

        let mut headers = HashMap::new();

        loop {
            line_buf.clear();

            let bytes_read = reader.read_line(&mut line_buf)?;

            // Если соединение закрылось или встретили пустую строку "\r\n" — заголовки закончились
            if bytes_read == 0 || line_buf == "\r\n" || line_buf == "\n" {
                break;
            }

            let (name, val) = line_buf
                .split_once(':')
                .ok_or(HttpParseError::InvalidHeader)?;

            headers.insert(name.trim().to_ascii_lowercase(), val.trim().to_string());
        }

        Ok(HttpRequestHead {
            method,
            path,
            version,
            headers,
        })
    }
}

pub fn parse_multipart_file<R: BufRead>(
    reader: &mut R,
    boundary: &str,
    upload_dir: &Path,
) -> Result<UploadedFile, HttpParseError> {
    std::fs::create_dir_all(upload_dir)?;

    let mut line = String::new();
    let initial_boundary = format!("--{boundary}");

    // Фаза 1: Поиск стартовой границы
    let mut found_boundary = false;
    while reader.read_line(&mut line)? > 0 {
        if line.trim() == initial_boundary {
            found_boundary = true;
            break;
        }
        line.clear();
    }

    if !found_boundary {
        return Err(HttpParseError::MultipartError(
            "Начальный boundary не найден".into(),
        ));
    }

    // Фаза 2: Чтение заголовков секции файла
    let mut field_name = String::from("file");
    let mut original_name = String::from("unnamed.bin");

    line.clear();
    while reader.read_line(&mut line)? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }

        if let Some((k, v)) = trimmed.split_once(':') {
            if k.trim().eq_ignore_ascii_case("Content-Disposition") {
                for param in v.split(';') {
                    if let Some((pk, pv)) = param.trim().split_once('=') {
                        let val = pv.trim().trim_matches('"');
                        if pk.trim().eq_ignore_ascii_case("name") {
                            field_name = val.to_string();
                        } else if pk.trim().eq_ignore_ascii_case("filename") {
                            if let Some(file_name_only) = Path::new(val).file_name() {
                                original_name = file_name_only.to_string_lossy().to_string();
                            }
                        }
                    }
                }
            }
        }
        line.clear();
    }

    // Фаза 3: Потоковое сохранение через fill_buf и consume
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let safe_disk_filename = format!("{}_{}", timestamp, original_name);
    let disk_path = upload_dir.join(safe_disk_filename);

    // Оборачиваем файл в BufWriter, чтобы побайтовый сброс не делал лишних системных вызовов
    let mut dest_file = BufWriter::new(File::create(&disk_path)?);

    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let delim_len = delimiter.len();

    let mut matched = 0; // Сколько байт границы совпало на стыке
    let mut total_bytes: u64 = 0;

    'stream_loop: loop {
        // 1. Получаем срез байт НАПРЯМУЮ из внутреннего буфера BufReader
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Err(HttpParseError::ConnectionClosed);
        }

        let mut consumed_bytes = 0;

        let mut start_piece_of_file = 0;

        // 2. Идем по байтам буфера через конечный автомат
        for (i, &byte) in buffer.iter().enumerate() {
            consumed_bytes += 1;

            if byte == delimiter[matched] {
                if matched == 0 && i > start_piece_of_file {
                    dest_file.write_all(&buffer[start_piece_of_file..i])?;
                    total_bytes += (i - start_piece_of_file + 1) as u64;
                }

                // Байт совпал со следующим символом границы!
                matched += 1;
                if matched == delim_len {
                    // Граница найдена полностью и отправляем ридеру сколкьо байт мы прочитали
                    reader.consume(consumed_bytes);
                    break 'stream_loop;
                }
            } else if matched > 0 {
                // часть байт совпала с началом границы, но следующий байт не подошел.
                // Значит, это были просто байты файла. Сбрасываем их на диск:
                dest_file.write_all(&delimiter[..matched])?;
                total_bytes += matched as u64;

                // Проверяем текущий байт: не начинает ли он границу заново?
                if byte == delimiter[0] {
                    matched = 1;
                    start_piece_of_file = i + 1;
                } else {
                    matched = 0;
                    start_piece_of_file = i;
                }
            }
        }

        if matched == 0 {
            if consumed_bytes > start_piece_of_file {
                dest_file.write_all(&buffer[start_piece_of_file..consumed_bytes])?;
                total_bytes += (consumed_bytes - start_piece_of_file) as u64;
            }
        } else {
            let match_start = consumed_bytes - matched;
            if match_start > start_piece_of_file {
                dest_file.write_all(&buffer[start_piece_of_file..match_start])?;
                total_bytes += (match_start - start_piece_of_file) as u64;
            }
        }

        // Вызываем consume на весь обработанный объем
        // Теперь буфер BufReader пуст, и следующий вызов fill_buf() гарантированно подгрузит новую порцию из сети
        reader.consume(consumed_bytes);
    }

    dest_file.flush()?;

    // Дочитываем финальный перенос строки или маркер `--\r\n`
    let mut tail = String::new();
    let _ = reader.read_line(&mut tail);

    Ok(UploadedFile {
        field_name,
        original_name,
        disk_path,
        size_bytes: total_bytes,
    })
}

fn extract_boundary(content_type: &str) -> Option<&str> {
    let mut parts = content_type.split(';');

    let mime_type = parts.next()?.trim();
    if !mime_type.eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }

    for param in parts {
        if let Some((key, val)) = param.split_once('=') {
            if key.trim().eq_ignore_ascii_case("boundary") {
                return Some(val.trim().trim_matches('"'));
            }
        }
    }
    None
}

fn handle_connection(stream: TcpStream) -> Result<(), HttpParseError> {
    let mut reader = BufReader::new(stream);
    match HttpRequestHead::from_reader(&mut reader) {
        Ok(head) => {
            println!("Запрос: {:?} {}", head.method, head.path);

            if head.method == HttpMethod::Post && head.path == "/upload" {
                let content_type = head
                    .headers
                    .get("content-type")
                    .cloned()
                    .unwrap_or_default();

                if let Some(boundary) = extract_boundary(&content_type) {
                    let upload_dir = Path::new("./uploads");

                    match parse_multipart_file(&mut reader, &boundary, upload_dir) {
                        Ok(file) => {
                            println!("Файл сохранен: {:?}", file);
                            let msg = format!(
                                "Файл '{}' ({} байт) успешно сохранен как {:?}",
                                file.original_name, file.size_bytes, file.disk_path
                            );
                            send_response(reader.get_mut(), "200 OK", &msg)?;
                        }
                        Err(e) => {
                            eprintln!("Ошибка загрузки файла: {:?}", e);
                            send_response(
                                reader.get_mut(),
                                "400 BAD REQUEST",
                                "Ошибка обработки файла",
                            )?;
                        }
                    }
                } else {
                    send_response(
                        reader.get_mut(),
                        "400 BAD REQUEST",
                        "Ожидался multipart/form-data",
                    )?;
                }
            } else if head.method == HttpMethod::Get && head.path == "/" {
                // Простая HTML-форма для тестирования
                let form_html = r#"<!DOCTYPE html>
<html>
<body>
    <h2>Загрузка файла</h2>
    <form action="/upload" method="post" enctype="multipart/form-data">
        <input type="file" name="file" required />
        <button type="submit">Отправить</button>
    </form>
</body>
</html>"#;
                send_response_html(reader.get_mut(), "200 OK", form_html)?;
            } else {
                send_response(reader.get_mut(), "404 NOT FOUND", "Маршрут не найден")?;
            }
        }
        Err(HttpParseError::ConnectionClosed) => {}
        Err(e) => {
            eprintln!("Ошибка парсинга заголовков: {:?}", e);
            let _ = send_response(reader.get_mut(), "400 BAD REQUEST", "Bad Request");
        }
    }

    Ok(())
}

fn send_response(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.as_bytes().len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn send_response_html(stream: &mut TcpStream, status: &str, html: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        html.as_bytes().len(),
        html
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8080").expect("Could not bind");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle_connection(s) {
                    match e {
                        HttpParseError::ConnectionClosed => {}
                        other => eprintln!("connection error: {:?}", other),
                    }
                }
            }
            Err(e) => { /* connection failed */ }
        }
    }
    Ok(())
}
