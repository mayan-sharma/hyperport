# Raw Socket TCP Server: Low-Level Implementation Explained

This document explains the complete custom TCP server implementation using raw POSIX system calls, bypassing all of Rust's standard library networking abstractions.

## Overview

Our HTTP server operates at the lowest possible userspace level, directly interfacing with the Linux kernel's network stack through POSIX system calls. No `std::net` abstractions are used.

## Memory Layout and Data Structures

### File Descriptors

```rust
struct RawTcpStream {
    fd: RawFd,  // i32 - kernel file descriptor number
}

struct CustomTcpListener {
    fd: RawFd,  // i32 - kernel file descriptor number
}
```

**What happens in memory:**

- `RawFd` is just an `i32` that references a kernel data structure
- The kernel maintains a **file descriptor table** per process
- Each FD points to a **socket structure** in kernel memory containing:
  - Socket state (LISTENING, ESTABLISHED, etc.)
  - Network protocol information
  - Buffer queues for incoming/outgoing data
  - Connection metadata

## System Call Deep Dive

### 1. Socket Creation (`socket()`)

```rust
let fd = unsafe {
    libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0)
};
```

**What this does at the kernel level:**

1. **System call transition**: CPU switches from user mode to kernel mode
2. **Kernel allocates memory** for a new socket structure:
   ```c
   struct socket {
       socket_state state;      // SS_UNCONNECTED initially
       short type;              // SOCK_STREAM
       unsigned long flags;
       struct proto_ops *ops;   // TCP protocol operations
       struct file *file;       // File system representation
       struct sock *sk;         // Protocol-specific socket info
   }
   ```
3. **File descriptor allocation**: Kernel finds the lowest available FD number
4. **Return to userspace**: FD number returned as integer

**Parameters explained:**

- `AF_INET`: IPv4 address family
- `SOCK_STREAM`: Reliable, connection-oriented protocol (TCP)
- `0`: Let kernel choose protocol (TCP for SOCK_STREAM)

### 2. Socket Options (`setsockopt()`)

```rust
libc::setsockopt(
    fd,                                    // Socket file descriptor
    libc::SOL_SOCKET,                     // Socket level (not protocol specific)
    libc::SO_REUSEADDR,                   // Option name
    &reuse as *const i32 as *const libc::c_void,  // Option value (1)
    mem::size_of::<i32>() as libc::socklen_t,     // Value size (4 bytes)
)
```

**Memory operations:**

1. **User-to-kernel data copy**: The `reuse` variable (value `1`) is copied from user memory to kernel memory
2. **Socket structure modification**: Kernel sets the `SO_REUSEADDR` flag in the socket's option flags
3. **Effect**: Allows binding to an address even if it's in `TIME_WAIT` state from previous connections

### 3. Address Binding (`bind()`)

```rust
let mut sockaddr_in: libc::sockaddr_in = unsafe { mem::zeroed() };
sockaddr_in.sin_family = libc::AF_INET as u16;     // 2 bytes
sockaddr_in.sin_port = addr.port().to_be();        // 2 bytes, big-endian
sockaddr_in.sin_addr.s_addr = u32::from(*addr.ip()).to_be(); // 4 bytes, big-endian

libc::bind(
    fd,
    &sockaddr as *const libc::sockaddr_in as *const libc::sockaddr,
    mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
)
```

**Memory layout of `sockaddr_in`:**

```
+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
| sin_family|  sin_port     |        sin_addr (IP)          |pad|
+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
  0   1   2   3   4   5   6   7   8   9  10  11  12  13  14  15
```

**Kernel operations:**

1. **Address validation**: Kernel checks if the IP/port combination is available
2. **Network interface binding**: Associates socket with network interface
3. **Port table update**: Kernel's port allocation table is updated
4. **Socket state change**: Socket moves to `SS_UNCONNECTED` but bound state

**Endianness conversion:**

- `to_be()` converts from host byte order to network byte order (big-endian)
- Network protocols always use big-endian for multi-byte values
- Intel x86/x64 is little-endian, so conversion is necessary

### 4. Listen Queue Setup (`listen()`)

```rust
if libc::listen(fd, 128) < 0 {
    // Error handling
}
```

**Kernel changes:**

1. **Socket state transition**: `SS_UNCONNECTED` → `SS_LISTENING`
2. **Accept queue allocation**: Kernel allocates memory for:
   - **SYN queue**: Half-open connections (SYN received, SYN-ACK sent)
   - **Accept queue**: Full connections waiting for `accept()` call
3. **Backlog limit**: Maximum 128 pending connections
4. **TCP state**: Socket can now receive SYN packets

### 5. Connection Acceptance (`accept()`)

```rust
let mut client_addr: libc::sockaddr_in = unsafe { mem::zeroed() };
let mut addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

let client_fd = unsafe {
    libc::accept(
        self.fd,
        &mut client_addr as *mut libc::sockaddr_in as *mut libc::sockaddr,
        &mut addr_len,
    )
};
```

**Complex kernel operations:**

1. **Queue check**: Kernel checks accept queue for completed connections
2. **If queue empty**: `accept()` blocks (sleeps) until connection arrives
3. **New socket creation**: Kernel creates a **new socket structure** for the client connection
4. **File descriptor allocation**: New FD assigned for client socket
5. **Address copying**: Client's IP/port copied to `client_addr` structure
6. **TCP state**: New socket is in `ESTABLISHED` state

**Memory flow:**

```
Listen Socket (fd=3)     Client Socket (fd=4)
┌─────────────────┐     ┌─────────────────┐
│ State: LISTENING│────▶│ State: ESTABLISHED│
│ Queue: [conn1]  │     │ Remote: 192.168.1.100:54321│
│ Backlog: 128    │     │ Local: 127.0.0.1:8080│
└─────────────────┘     └─────────────────┘
```

## Data Transfer Operations

### 6. Reading Data (`read()`)

```rust
let bytes_read = unsafe {
    libc::read(
        self.fd,
        buf.as_mut_ptr() as *mut libc::c_void,
        buf.len(),
    )
};
```

**Kernel operations:**

1. **Buffer check**: Kernel checks socket's receive buffer for data
2. **If no data**: Process may block until data arrives
3. **Memory copy**: Data copied from kernel socket buffer to user buffer
4. **Buffer management**: Kernel updates receive buffer pointers
5. **Return value**: Number of bytes actually copied

**Memory diagram:**

```
Kernel Space                    User Space
┌─────────────────┐            ┌─────────────────┐
│ Socket RX Buffer│   copy     │ User Buffer     │
│ [HTTP request]  │  ────────▶ │ [0; 1024]       │
│ Size: 1024 bytes│            │ Size: 1024 bytes│
└─────────────────┘            └─────────────────┘
```

### 7. Writing Data (`write()`)

```rust
let bytes_written = unsafe {
    libc::write(
        self.fd,
        buf[total_written..].as_ptr() as *const libc::c_void,
        buf.len() - total_written,
    )
};
```

**Kernel operations:**

1. **Buffer space check**: Kernel checks available space in socket's send buffer
2. **Memory copy**: Data copied from user buffer to kernel socket buffer
3. **TCP transmission**: Kernel's TCP stack sends data as TCP segments
4. **Return value**: Number of bytes accepted into kernel buffer (may be less than requested)

**Why we need a loop:**

- `write()` is **not guaranteed** to write all data in one call
- Kernel send buffer might be full
- Network congestion might limit transmission
- Our `write_all()` ensures all data is eventually sent

## HTTP Protocol Handling

### Request Parsing

```rust
fn parse_request(request: &str) -> Result<(String, String), &'static str> {
    let request_line = lines[0];                    // "GET /path HTTP/1.1"
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts[0].to_string();              // "GET"
    let path = parts[1].to_string();                // "/path"
}
```

**Raw HTTP request in memory:**

```
Bytes in buffer: [0x47, 0x45, 0x54, 0x20, 0x2F, ...]
As string:       "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"
                  └─┬─┘ └┬┘ └──┬───┘
                  method path version
```

### Response Generation

```rust
let response = format!(
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
    html_body.len(),
    html_body
);
```

**HTTP response structure:**

```
HTTP/1.1 200 OK\r\n                    ← Status line
Content-Type: text/html; charset=utf-8\r\n  ← Headers
Connection: close\r\n
Content-Length: 118\r\n
\r\n                                   ← Empty line separates headers from body
<!DOCTYPE html>...                     ← Body
```

## Memory Management

### Resource Cleanup

```rust
impl Drop for RawTcpStream {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);  // System call to kernel
        }
    }
}
```

**Kernel cleanup operations:**

1. **Socket structure deallocation**: Kernel frees socket memory
2. **File descriptor release**: FD number becomes available for reuse
3. **Network cleanup**: Any pending TCP packets are handled
4. **Connection termination**: TCP FIN packets sent if connection active

## Threading and Concurrency

```rust
thread::spawn(|| {
    handle_connection(stream);
});
```

**OS-level operations:**

1. **Thread creation**: OS creates new thread with its own stack
2. **Socket ownership transfer**: File descriptor is moved to new thread
3. **Concurrent processing**: Multiple connections handled simultaneously
4. **Resource isolation**: Each thread has independent stack and registers

## Performance Characteristics

### System Call Overhead

- Each system call involves **user↔kernel mode switch**
- CPU must save/restore registers and memory mappings
- Typical overhead: ~100-300 CPU cycles per system call

### Memory Copies

- Data is copied multiple times:
  1. Network hardware → Kernel buffers
  2. Kernel buffers → User space buffers
  3. User space → String processing

### Blocking vs Non-blocking

- Our implementation uses **blocking I/O**
- Thread blocks on `accept()` and `read()` until data available
- Alternative: **epoll/kqueue** for non-blocking I/O multiplexing

## Comparison with High-Level Libraries

| Level                        | What it abstracts       | Performance     | Complexity      |
| ---------------------------- | ----------------------- | --------------- | --------------- |
| Our implementation           | Nothing - raw syscalls  | Highest control | High complexity |
| `std::net::TcpListener`      | Socket creation/binding | Slight overhead | Medium          |
| HTTP frameworks (axum, warp) | Protocol parsing        | Higher overhead | Low complexity  |

## Security Considerations

### Buffer Overflows

- Fixed-size buffer `[0; 1024]` limits request size
- Malicious clients could send larger requests
- Production code needs dynamic buffer management

### Resource Exhaustion

- No connection limits implemented
- Malicious clients could exhaust file descriptors
- Need connection pooling and rate limiting

### Memory Safety

- Heavy use of `unsafe` blocks for system calls
- Rust's ownership prevents many bugs
- Manual resource management in `Drop` implementations

This implementation demonstrates the fundamental building blocks that all network programming libraries use internally, providing maximum control at the cost of complexity.
