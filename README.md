# Hyperport

A simple HTTP server implementation written in Rust that demonstrates basic web server functionality.

## Features

- TCP connection handling
- HTTP request parsing
- Multi-threaded connection processing
- Basic HTTP responses (200 OK, 400 Bad Request)

## Usage

Run the server:
```bash
cargo run
```

The server will start on `http://127.0.0.1:8080` and serve a simple "Hello, World!" page.

## Building

```bash
cargo build
```

## Testing

You can test the server with curl:
```bash
curl http://127.0.0.1:8080
```

## Implementation Details

- Uses `std::net::TcpListener` for accepting connections
- Spawns a new thread for each connection
- Parses basic HTTP request format (method and path)
- Returns HTML responses with proper HTTP headers