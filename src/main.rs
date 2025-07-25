use std::io::prelude::*;
use std::net::{TcpListener, TcpStream};
use std::thread;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:8080").unwrap();
    println!("Server running on http://127.0.0.1:8080");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(|| {
                    handle_connection(stream);
                });
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
}

fn handle_connection(mut stream: TcpStream) {
    let mut buffer = [0; 1024];
    
    match stream.read(&mut buffer) {
        Ok(_) => {
            let request = String::from_utf8_lossy(&buffer[..]);
            
            match parse_request(&request) {
                Ok((method, path)) => {
                    println!("Request: {} {}", method, path);
                    send_ok_response(&mut stream);
                }
                Err(_) => {
                    send_bad_request_response(&mut stream);
                }
            }
        }
        Err(e) => {
            eprintln!("Error reading from stream: {}", e);
        }
    }
}

fn parse_request(request: &str) -> Result<(String, String), &'static str> {
    let lines: Vec<&str> = request.lines().collect();
    if lines.is_empty() {
        return Err("Empty request");
    }
    
    let request_line = lines[0];
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    
    if parts.len() < 3 {
        return Err("Invalid request line");
    }
    
    let method = parts[0].to_string();
    let path = parts[1].to_string();
    
    Ok((method, path))
}

fn send_ok_response(stream: &mut TcpStream) {
    let html_body = r#"<!DOCTYPE html>
<html>
<head>
    <title>Hello World</title>
</head>
<body>
    <h1>Hello, World!</h1>
</body>
</html>"#;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        html_body.len(),
        html_body
    );

    if let Err(e) = stream.write_all(response.as_bytes()) {
        eprintln!("Error writing response: {}", e);
    }
}

fn send_bad_request_response(stream: &mut TcpStream) {
    let html_body = r#"<!DOCTYPE html>
<html>
<head>
    <title>Bad Request</title>
</head>
<body>
    <h1>400 Bad Request</h1>
</body>
</html>"#;

    let response = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        html_body.len(),
        html_body
    );

    if let Err(e) = stream.write_all(response.as_bytes()) {
        eprintln!("Error writing bad request response: {}", e);
    }
}