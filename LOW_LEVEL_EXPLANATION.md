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

## TCP State Machine Deep Dive

### Connection State Transitions

TCP connections follow a complex state machine. Here's what happens during our server's lifecycle:

```
Server Socket States:
CLOSED → BIND → LISTEN → ACCEPT → ESTABLISHED → CLOSE_WAIT → CLOSED

Client Connection States (per connection):
SYN_SENT → SYN_RECEIVED → ESTABLISHED → FIN_WAIT_1 → FIN_WAIT_2 → TIME_WAIT → CLOSED
```

### Detailed State Transitions

#### Server Listening Process

```rust
// 1. CLOSED → BIND
libc::socket()     // Socket created in CLOSED state
libc::bind()       // Still CLOSED but bound to address

// 2. BIND → LISTEN  
libc::listen()     // Transitions to LISTEN state
```

**Kernel state changes:**
- Socket marked as passive (accepting connections)
- Accept queue allocated
- SYN queue allocated for half-open connections

#### Three-Way Handshake (Accept Process)

```
Client                    Server
  |                         |
  |  SYN (seq=100)         |
  |──────────────────────▶ |  (Server socket stays LISTEN)
  |                         |  (New socket created in SYN_RECEIVED)
  |  SYN-ACK (seq=200,     |
  |           ack=101)     |
  |◀──────────────────────  |
  |                         |
  |  ACK (ack=201)         |
  |──────────────────────▶ |  (New socket moves to ESTABLISHED)
  |                         |  (Added to accept queue)
```

**What `accept()` actually does:**
1. Checks accept queue for completed connections
2. If empty, blocks until 3-way handshake completes
3. Returns new file descriptor for ESTABLISHED connection
4. Original listen socket remains in LISTEN state

#### Connection Termination

```
Client                    Server
  |                         |
  |  FIN (seq=300)         |
  |──────────────────────▶ |  (ESTABLISHED → CLOSE_WAIT)
  |                         |
  |  ACK (ack=301)         |
  |◀──────────────────────  |
  |                         |
  |  FIN (seq=400)         |
  |◀──────────────────────  |  (CLOSE_WAIT → LAST_ACK)
  |                         |
  |  ACK (ack=401)         |
  |──────────────────────▶ |  (LAST_ACK → CLOSED)
```

### Kernel Memory Layout for TCP Connections

#### Socket Structure Hierarchy

```c
// Simplified kernel structures
struct socket {
    short                  type;        // SOCK_STREAM
    socket_state           state;       // SS_CONNECTING, SS_CONNECTED, etc.
    struct proto_ops      *ops;        // TCP operations table
    struct sock           *sk;         // Protocol-specific data
    struct file           *file;       // VFS file structure
    unsigned long          flags;      // Socket flags (SO_REUSEADDR, etc.)
};

struct sock {
    // Network layer info
    __be32                 saddr;      // Source IP (big-endian)
    __be32                 daddr;      // Destination IP
    __be16                 sport;      // Source port
    __be16                 dport;      // Destination port
    
    // TCP-specific data  
    struct tcp_sock       *tcp;        // TCP state machine
    
    // Buffer management
    struct sk_buff_head    receive_queue;  // Incoming data
    struct sk_buff_head    write_queue;    // Outgoing data
    
    // Memory accounting
    int                    sndbuf;     // Send buffer size
    int                    rcvbuf;     // Receive buffer size
    atomic_t               rmem_alloc; // Allocated receive memory
    atomic_t               wmem_alloc; // Allocated send memory
};

struct tcp_sock {
    // State machine
    u8                     state;      // TCP_LISTEN, TCP_ESTABLISHED, etc.
    
    // Sequence numbers
    u32                    snd_nxt;    // Next sequence number to send
    u32                    snd_una;    // Unacknowledged sequence number
    u32                    rcv_nxt;    // Next expected sequence number
    
    // Window management
    u32                    snd_wnd;    // Send window size
    u32                    rcv_wnd;    // Receive window size
    
    // Congestion control
    u32                    snd_cwnd;   // Congestion window
    u32                    snd_ssthresh; // Slow start threshold
    
    // Retransmission
    struct timer_list      retransmit_timer;
    u32                    rto;        // Retransmission timeout
};
```

## Network Packet Flow Analysis

### Incoming HTTP Request Journey

```
1. Network Interface Card (NIC)
   │ Receives Ethernet frame
   │ DMA transfer to ring buffer
   └─▶ Hardware interrupt

2. Interrupt Handler
   │ Processes Ethernet header
   │ Extracts IP packet
   └─▶ IP layer processing

3. IP Layer
   │ Validates IP header checksum
   │ Checks destination IP
   │ Extracts TCP segment
   └─▶ TCP layer processing

4. TCP Layer
   │ Validates TCP checksum
   │ Finds socket by (src_ip, src_port, dst_ip, dst_port)
   │ Processes TCP flags and sequence numbers
   │ Updates TCP state machine
   └─▶ Socket buffer

5. Socket Buffer (sk_buff)
   │ Data queued in socket's receive queue
   │ Process sleeping on read() is awakened
   └─▶ User space via read() system call
```

### TCP Segment Structure

```
 0                   1                   2                   3   
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Source Port          |       Destination Port        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Sequence Number                        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    Acknowledgment Number                      |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|  Data |           |U|A|P|R|S|F|                               |
| Offset| Reserved  |R|C|S|S|Y|I|            Window             |
|       |           |G|K|H|T|N|N|                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|           Checksum            |         Urgent Pointer        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    Options                    |    Padding    |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                             data                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

For our HTTP request "GET / HTTP/1.1\r\n\r\n":
- **Source Port**: Client's ephemeral port (e.g., 54321)
- **Dest Port**: Our server port (8080)
- **Sequence Number**: Client's current sequence number
- **ACK Number**: Expected next byte from server
- **Flags**: PSH+ACK (0x18) - Push data and acknowledge
- **Data**: "GET / HTTP/1.1\r\nHost: localhost:8080\r\n\r\n"

### Outgoing HTTP Response Journey

```
1. User Space write() Call
   │ HTTP response string in user buffer
   │ System call copies data to kernel
   └─▶ Socket send buffer

2. TCP Layer Processing  
   │ Breaks data into MSS-sized segments
   │ Adds TCP headers (sequence numbers, etc.)
   │ Applies congestion control
   └─▶ IP layer

3. IP Layer Processing
   │ Adds IP header
   │ Handles fragmentation if needed
   │ Routes packet via routing table
   └─▶ Network interface

4. Network Interface
   │ Adds Ethernet header
   │ Queues packet in TX ring buffer
   │ DMA transfer to NIC
   └─▶ Physical transmission
```

## Advanced Kernel Internals

### Memory Management Deep Dive

#### Socket Buffer (sk_buff) Structure

The kernel uses `sk_buff` structures to manage network packets:

```c
struct sk_buff {
    struct sk_buff         *next;      // Linked list pointer
    struct sk_buff         *prev;
    struct sock            *sk;        // Owning socket
    
    unsigned int           len;        // Data length
    unsigned int           data_len;   // Non-linear data length
    __u16                  mac_len;    // MAC header length
    __u16                  hdr_len;    // Writable header length
    
    // Data pointers
    sk_buff_data_t         tail;       // End of data
    sk_buff_data_t         end;        // End of buffer
    unsigned char          *head;      // Buffer start
    unsigned char          *data;      // Data start
    
    // Reference counting
    atomic_t               users;      // Reference count
    
    // Network headers
    struct tcphdr          *th;        // TCP header
    struct iphdr           *iph;       // IP header
};
```

#### Buffer Management During read()

```rust
// Our read() call
let bytes_read = unsafe {
    libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
};
```

**Kernel operations:**

1. **Socket lookup**: Find socket structure from file descriptor
2. **Receive queue check**: Examine `sk->receive_queue` for data
3. **sk_buff processing**: 
   - Traverse linked list of sk_buff structures
   - Extract data from linear and non-linear portions
   - Handle TCP sequence number ordering
4. **Memory copy**: `copy_to_user()` from kernel space to user buffer
5. **Buffer cleanup**: Release processed sk_buff structures
6. **Flow control**: Update TCP window advertisements

#### Zero-Copy Optimizations (Not Used in Our Implementation)

Modern kernels support zero-copy techniques:

- **sendfile()**: Direct file-to-socket transfer
- **splice()**: Move data between file descriptors
- **mmap()**: Memory-mapped I/O
- **MSG_ZEROCOPY**: Avoid copying user data

Our implementation uses traditional copy-based I/O for simplicity.

### File Descriptor Table Management

#### Process File Descriptor Table

```c
struct files_struct {
    atomic_t        count;              // Reference count
    struct fdtable  *fdt;              // File descriptor table
    spinlock_t      file_lock;         // Synchronization
    int             next_fd;           // Next available FD
};

struct fdtable {
    unsigned int    max_fds;           // Maximum FDs
    struct file     **fd;              // Array of file pointers
    unsigned long   *close_on_exec;    // Close-on-exec bitmap
    unsigned long   *open_fds;         // Open FD bitmap
};
```

**FD allocation for our socket:**

1. `socket()` creates `struct file` and `struct socket`
2. Kernel finds lowest available FD number
3. `files_struct->fdt->fd[n]` points to our socket's file structure
4. Returns `n` as file descriptor to userspace

### Interrupt Handling and Network Processing

#### Hardware Interrupt Flow

```
1. NIC receives packet
   │ Stores in ring buffer via DMA
   │ Raises hardware interrupt
   └─▶ CPU interrupt handler

2. Interrupt Handler (hardirq context)
   │ Minimal processing (can't sleep)
   │ Schedules software interrupt (NET_RX_SOFTIRQ)
   │ Acknowledges hardware interrupt
   └─▶ Returns quickly

3. Softirq Handler (softirq context) 
   │ Processes packets from ring buffer
   │ Parses headers (Ethernet, IP, TCP)
   │ Delivers to appropriate socket
   └─▶ Wakes up sleeping processes

4. Process Context
   │ read() system call returns
   │ User space receives data
   └─▶ HTTP request processing
```

#### NAPI (New API) Processing

Modern network drivers use NAPI for efficiency:

```c
// Simplified NAPI processing
static int our_driver_poll(struct napi_struct *napi, int budget) {
    int packets_processed = 0;
    
    while (packets_processed < budget) {
        struct sk_buff *skb = get_packet_from_ring_buffer();
        if (!skb) break;
        
        netif_receive_skb(skb);  // Send to network stack
        packets_processed++;
    }
    
    if (packets_processed < budget) {
        napi_complete(napi);      // Re-enable interrupts
    }
    
    return packets_processed;
}
```

## Error Handling and Edge Cases

### System Call Error Conditions

#### Socket Creation Errors

```rust
let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
if fd < 0 {
    let errno = std::io::Error::last_os_error();
    match errno.raw_os_error() {
        Some(libc::EAFNOSUPPORT) => // Address family not supported
        Some(libc::EPROTONOSUPPORT) => // Protocol not supported  
        Some(libc::EMFILE) => // Process file descriptor limit reached
        Some(libc::ENFILE) => // System file descriptor limit reached
        Some(libc::ENOBUFS) => // Insufficient memory
        _ => // Other errors
    }
}
```

#### Bind Errors

```rust
if unsafe { libc::bind(fd, addr_ptr, addr_len) } < 0 {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EADDRINUSE) => // Address already in use
        Some(libc::EACCES) => // Permission denied (port < 1024 needs root)
        Some(libc::EADDRNOTAVAIL) => // Address not available
        Some(libc::EBADF) => // Invalid file descriptor
        Some(libc::EINVAL) => // Socket already bound
        Some(libc::ENOTSOCK) => // FD is not a socket
        _ => // Other errors
    }
}
```

#### Accept Errors

```rust
let client_fd = unsafe { libc::accept(fd, addr_ptr, addr_len_ptr) };
if client_fd < 0 {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EAGAIN) | Some(libc::EWOULDBLOCK) => // No connections (non-blocking)
        Some(libc::EBADF) => // Invalid file descriptor
        Some(libc::ECONNABORTED) => // Connection aborted
        Some(libc::EINTR) => // Interrupted by signal
        Some(libc::EINVAL) => // Socket not listening
        Some(libc::EMFILE) => // Process FD limit reached
        Some(libc::ENFILE) => // System FD limit reached
        Some(libc::ENOBUFS) => // Insufficient memory
        Some(libc::EPROTO) => // Protocol error
        _ => // Other errors
    }
}
```

### Connection Handling Edge Cases

#### Partial Reads

```rust
fn read_http_request(stream: &mut RawTcpStream) -> Result<String, std::io::Error> {
    let mut buffer = Vec::new();
    let mut temp_buf = [0u8; 1024];
    
    loop {
        match stream.read(&mut temp_buf) {
            Ok(0) => break, // EOF - client closed connection
            Ok(n) => {
                buffer.extend_from_slice(&temp_buf[..n]);
                
                // Check for complete HTTP request
                if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                
                // Prevent DoS - limit request size
                if buffer.len() > 8192 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Request too large"
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Non-blocking socket with no data
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                // System call interrupted by signal
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    
    String::from_utf8(buffer).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8")
    })
}
```

#### Partial Writes

```rust
fn write_all(&mut self, mut buf: &[u8]) -> Result<(), std::io::Error> {
    while !buf.is_empty() {
        match unsafe {
            libc::write(
                self.fd,
                buf.as_ptr() as *const libc::c_void,
                buf.len(),
            )
        } {
            -1 => {
                let error = std::io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EINTR) => continue, // Interrupted, retry
                    Some(libc::EAGAIN) | Some(libc::EWOULDBLOCK) => {
                        // Send buffer full, would block
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                    Some(libc::EPIPE) => {
                        // Broken pipe - client disconnected
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "Client disconnected"
                        ));
                    }
                    Some(libc::ECONNRESET) => {
                        // Connection reset by peer
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::ConnectionReset,
                            "Connection reset by peer"
                        ));
                    }
                    _ => return Err(error),
                }
            }
            0 => {
                // Should not happen with write()
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "Write returned 0"
                ));
            }
            n => {
                buf = &buf[n as usize..];
            }
        }
    }
    Ok(())
}
```

### Signal Handling

#### SIGPIPE Handling

```rust
// Ignore SIGPIPE to handle broken pipes gracefully
unsafe {
    libc::signal(libc::SIGPIPE, libc::SIG_IGN);
}
```

Without this, writing to a closed socket would terminate our process with SIGPIPE.

#### Graceful Shutdown

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn setup_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as *const () as usize);
        libc::signal(libc::SIGTERM, handle_signal as *const () as usize);
    }
}

extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn main() {
    setup_signal_handlers();
    
    while !SHUTDOWN.load(Ordering::SeqCst) {
        // Accept connections
        match listener.accept() {
            Ok(stream) => {
                thread::spawn(move || {
                    if !SHUTDOWN.load(Ordering::SeqCst) {
                        handle_connection(stream);
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                // accept() interrupted by signal
                continue;
            }
            Err(e) => eprintln!("Accept error: {}", e),
        }
    }
}
```

## Performance Analysis and Optimization

### Benchmarking System Call Overhead

#### Measuring System Call Latency

```rust
use std::time::Instant;

fn benchmark_syscalls() {
    let iterations = 1_000_000;
    
    // Benchmark socket creation
    let start = Instant::now();
    for _ in 0..iterations {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        unsafe { libc::close(fd) };
    }
    let socket_time = start.elapsed();
    println!("socket(): {} ns/call", socket_time.as_nanos() / iterations);
    
    // Benchmark read() on empty socket
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    let mut buf = [0u8; 1];
    
    let start = Instant::now();
    for _ in 0..iterations {
        unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    }
    let read_time = start.elapsed();
    println!("read(): {} ns/call", read_time.as_nanos() / iterations);
    
    unsafe { libc::close(fd) };
}
```

**Typical results on modern x86_64:**
- `socket()`: ~2000-5000 ns
- `read()` (no data): ~500-1500 ns  
- `write()`: ~500-2000 ns
- Context switch overhead: ~1000-3000 ns

### Memory Allocation Patterns

#### Socket Buffer Memory Usage

```rust
fn analyze_memory_usage() {
    // Check socket buffer sizes
    let fd = create_socket();
    
    let mut sndbuf: libc::c_int = 0;
    let mut optlen = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    
    unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &mut sndbuf as *mut _ as *mut libc::c_void,
            &mut optlen,
        );
    }
    
    println!("Default send buffer size: {} bytes", sndbuf);
    
    // Typical values on Linux:
    // - Send buffer: 16384-65536 bytes
    // - Receive buffer: 65536-131072 bytes
    // - Total per connection: ~128KB-256KB
}
```

#### Memory Scaling with Connection Count

```
Connections │ Memory Usage (approx)
───────────┼─────────────────────
1          │ 256 KB (socket buffers)
100        │ 25.6 MB
1,000      │ 256 MB  
10,000     │ 2.56 GB
```

### CPU Utilization Patterns

#### Thread vs Process Model

```rust
// Thread-per-connection (our model)
fn thread_model_analysis() {
    // Pros:
    // - Simple programming model
    // - Good for CPU-bound work per connection
    // - Automatic load balancing
    
    // Cons:  
    // - Stack memory: 8MB per thread (default)
    // - Context switch overhead
    // - Limited by system thread limits (~32K threads)
    
    // Memory calculation:
    // 1000 connections = 1000 threads × 8MB = 8GB RAM (just for stacks!)
}

// Alternative: Event-driven model (epoll/kqueue)
fn event_driven_model() {
    // Pros:
    // - Scales to 100K+ connections
    // - Lower memory usage
    // - No context switch overhead between connections
    
    // Cons:
    // - Complex state management
    // - Single-threaded (unless using multiple event loops)
    // - Callback-based programming
}
```

### Network Performance Considerations

#### TCP Window Scaling

```rust
fn optimize_tcp_windows() {
    let fd = create_socket();
    
    // Increase socket buffers for high-bandwidth connections
    let large_buffer = 1024 * 1024; // 1MB
    
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &large_buffer as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        );
        
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &large_buffer as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        );
    }
    
    // Enable TCP window scaling (usually default on modern systems)
    // This allows TCP windows > 65535 bytes
}
```

#### Nagle's Algorithm Control

```rust
fn disable_nagle_algorithm(fd: RawFd) {
    let nodelay = 1i32;
    unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_NODELAY,
            &nodelay as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        );
    }
    
    // Effect: Send small packets immediately
    // Trade-off: Lower latency vs higher bandwidth efficiency
    // Good for: Interactive applications, HTTP/1.1 with small responses
    // Bad for: Bulk data transfer
}
```

### Profiling and Monitoring

#### System-Level Monitoring

```bash
# Monitor socket states
ss -tuln

# Monitor network interrupts  
cat /proc/interrupts | grep eth0

# Monitor TCP statistics
cat /proc/net/netstat | grep Tcp

# Monitor socket memory usage
cat /proc/net/sockstat

# Example output:
# sockets: used 1000
# TCP: inuse 800 orphan 0 tw 200 alloc 850 mem 64
# UDP: inuse 50 mem 8
```

#### Application-Level Profiling

```rust
use std::sync::atomic::{AtomicU64, Ordering};

static CONNECTIONS_ACCEPTED: AtomicU64 = AtomicU64::new(0);
static BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static BYTES_RECEIVED: AtomicU64 = AtomicU64::new(0);

fn log_statistics() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        
        let connections = CONNECTIONS_ACCEPTED.load(Ordering::Relaxed);
        let sent = BYTES_SENT.load(Ordering::Relaxed);
        let received = BYTES_RECEIVED.load(Ordering::Relaxed);
        
        println!("Stats: {} connections, {} bytes sent, {} bytes received", 
                 connections, sent, received);
    }
}

// In connection handler:
fn handle_connection(mut stream: RawTcpStream) {
    CONNECTIONS_ACCEPTED.fetch_add(1, Ordering::Relaxed);
    
    // Track bytes in read/write operations
    let bytes = stream.read(&mut buffer)?;
    BYTES_RECEIVED.fetch_add(bytes as u64, Ordering::Relaxed);
    
    let response = generate_response();
    stream.write_all(response.as_bytes())?;
    BYTES_SENT.fetch_add(response.len() as u64, Ordering::Relaxed);
}
```

This implementation demonstrates the fundamental building blocks that all network programming libraries use internally, providing maximum control at the cost of complexity.
