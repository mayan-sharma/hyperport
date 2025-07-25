use std::net::SocketAddr;
use std::thread;
use std::os::unix::io::RawFd;
use std::mem;

struct RawTcpStream {
    fd: RawFd,
}

impl RawTcpStream {
    fn from_raw_fd(fd: RawFd) -> Self {
        RawTcpStream { fd }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        let bytes_read = unsafe {
            libc::read(
                self.fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };

        if bytes_read < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(bytes_read as usize)
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), std::io::Error> {
        let mut total_written = 0;
        
        while total_written < buf.len() {
            let bytes_written = unsafe {
                libc::write(
                    self.fd,
                    buf[total_written..].as_ptr() as *const libc::c_void,
                    buf.len() - total_written,
                )
            };

            if bytes_written < 0 {
                return Err(std::io::Error::last_os_error());
            }

            total_written += bytes_written as usize;
        }

        Ok(())
    }
}

impl Drop for RawTcpStream {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

struct CustomTcpListener {
    fd: RawFd,
}

impl CustomTcpListener {
    fn bind(addr: &str) -> Result<Self, std::io::Error> {
        let socket_addr: SocketAddr = addr.parse().unwrap();
        
        let fd = unsafe {
            libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0)
        };
        
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let reuse = 1i32;
        unsafe {
            if libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                &reuse as *const i32 as *const libc::c_void,
                mem::size_of::<i32>() as libc::socklen_t,
            ) < 0 {
                libc::close(fd);
                return Err(std::io::Error::last_os_error());
            }
        }

        let sockaddr = match socket_addr {
            SocketAddr::V4(addr) => {
                let mut sockaddr_in: libc::sockaddr_in = unsafe { mem::zeroed() };
                sockaddr_in.sin_family = libc::AF_INET as u16;
                sockaddr_in.sin_port = addr.port().to_be();
                sockaddr_in.sin_addr.s_addr = u32::from(*addr.ip()).to_be();
                sockaddr_in
            }
            SocketAddr::V6(_) => panic!("IPv6 not supported in this example"),
        };

        unsafe {
            let bind_result = libc::bind(
                fd,
                &sockaddr as *const libc::sockaddr_in as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );
            
            if bind_result < 0 {
                libc::close(fd);
                return Err(std::io::Error::last_os_error());
            }

            if libc::listen(fd, 128) < 0 {
                libc::close(fd);
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(CustomTcpListener { fd })
    }

    fn accept(&self) -> Result<RawTcpStream, std::io::Error> {
        let mut client_addr: libc::sockaddr_in = unsafe { mem::zeroed() };
        let mut addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

        let client_fd = unsafe {
            libc::accept(
                self.fd,
                &mut client_addr as *mut libc::sockaddr_in as *mut libc::sockaddr,
                &mut addr_len,
            )
        };

        if client_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(RawTcpStream::from_raw_fd(client_fd))
    }
}

impl Drop for CustomTcpListener {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

fn main() {
    let listener = CustomTcpListener::bind("127.0.0.1:8080").unwrap();
    println!("Custom TCP Server running on http://127.0.0.1:8080");

    loop {
        match listener.accept() {
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

fn handle_connection(mut stream: RawTcpStream) {
    let mut buffer = [0; 1024];
    
    match stream.read(&mut buffer) {
        Ok(bytes_read) => {
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            
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

fn send_ok_response(stream: &mut RawTcpStream) {
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

fn send_bad_request_response(stream: &mut RawTcpStream) {
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