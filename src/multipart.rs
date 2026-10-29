use std::io::{BufRead, Write, BufWriter};
use std::path::{PathBuf, Path};
use std::fs::{File};
use std::time::{UNIX_EPOCH, SystemTime};

use crate::http::{HttpParseError};

#[derive(Debug)]
pub struct UploadedFile {
    pub field_name: String,
    pub original_name: String,
    pub disk_path: PathBuf,
    pub size_bytes: u64,
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

pub fn extract_boundary(content_type: &str) -> Option<&str> {
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