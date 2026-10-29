use std::str::FromStr;
use std::collections::HashMap;
use std::io::{self, BufReader, BufRead, Write};
use std::net::{TcpStream, };


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

pub fn send_response(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.as_bytes().len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

pub fn send_response_html(stream: &mut TcpStream, status: &str, html: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        html.as_bytes().len(),
        html
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
