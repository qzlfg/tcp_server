use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::Path;

mod http;
mod multipart;

use http::{HttpMethod, HttpRequestHead, HttpParseError, send_response, send_response_html};
use multipart::{extract_boundary, parse_multipart_file};


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
