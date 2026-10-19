use std::io::prelude::*;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, mpsc::channel};
use std::thread;

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    println!("Connetion from: {}", stream.peer_addr()?);
    let mut buf = [0; 1024];
    let bytes_read = stream.read(&mut buf)?;

    if bytes_read == 0 {
        return Ok(());
    }

    let raw_string = String::from_utf8_lossy(&buf[..bytes_read]);
    let message = raw_string.lines();

    for line in message {
        println!("{}", line)
    }

    let status = "HTTP/1.1 200 OK\r\n";
    let content_type = "Content-Type: text/plain\r\n";
    let body = "Hello from Rust!";
    let response = format!(
        "{}Content-Length: {}\r\n{}\r\n{}",
        status,
        body.len(),
        content_type,
        body
    );

    stream.write_all(response.as_bytes())?;
    stream.flush()?;
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

