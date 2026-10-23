use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::{Arc, Mutex, mpsc::channel};
use std::thread;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

#[derive(Debug)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub version: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
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
            _ => Err(HttpParseError::InvalidMethod(s.to_string())),
        }
    }
}

impl HttpRequest {
    pub fn from_stream(stream: &mut TcpStream) -> Result<Self, HttpParseError> {
        let mut reader = BufReader::new(stream);
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

        line_buf.clear();

        let mut headers = HashMap::new();

        loop {
            let bytes_read = reader.read_line(&mut line_buf)?;

            // Если соединение закрылось или встретили пустую строку "\r\n" — заголовки закончились
            if bytes_read == 0 || line_buf == "\r\n" || line_buf == "\n" {
                break;
            }

            if let Some((name, val)) = line_buf.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), val.trim().to_string());
            } else {
                return Err(HttpParseError::InvalidHeader);
            }

            line_buf.clear();
        }

        let mut body = Vec::new();

        if let Some(content_length_str) = headers.get("content-length") {
            let content_length: usize = content_length_str
                .parse()
                .map_err(|_| HttpParseError::InvalidContentLength)?;

            if content_length > 0 {
                body.resize(content_length, 0);

                reader.read_exact(&mut body)?;
            }
        }

        Ok(HttpRequest {
            method,
            path,
            version,
            headers,
            body,
        })
    }
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    match HttpRequest::from_stream(&mut stream) {
        Ok(req) => {
            println!("--- Получен запрос ---");
            println!("Метод: {:?}", req.method);
            println!("Путь: {}", req.path);
            println!("Заголовков: {}", req.headers.len());
            println!("Размер тела: {} байт", req.body.len());

            let (status, body) = match req.path.as_str() {
                "/" => ("HTTP/1.1 200 OK\r\n", "Главная страница"),
                "/ping" => ("HTTP/1.1 200 OK\r\n", "pong"),
                _ => ("HTTP/1.1 404 NOT FOUND\r\n", "Страница не найдена"),
            };

            let response = format!(
                "{}Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                status,
                body.len(),
                body
            );

            stream.write_all(response.as_bytes())?;
            stream.flush()?;
        }
        Err(HttpParseError::ConnectionClosed) => {}
        Err(e) => {
            eprintln!("Ошибка парсинга запроса: {:?}", e);
            let bad_request = "HTTP/1.1 400 BAD REQUEST\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(bad_request.as_bytes());
        }
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8080").expect("Could not bind");
    let (tx, rx) = channel();
    for i in 0..10 {
        let sensor_tx = tx.clone();

        thread::spawn(move || {
            sensor_tx.send(i).unwrap();
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                handle_connection(stream);
            }
            Err(e) => { /* connection failed */ }
        }
    }
    Ok(())
}
