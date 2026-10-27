//! What the kernel knows about a player's connection: its round trip, what
//! it sent and sent again, and what the player hasn't got yet; and how to
//! make it resend sooner. Only Linux has these; elsewhere they do nothing.

use std::net::TcpStream;
use std::time::Duration;

/// One reading of a connection's TCP state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcpState {
    /// The kernel's smoothed round trip.
    pub rtt: Duration,
    /// Bytes written but not sent yet.
    pub unsent: u32,
    /// Bytes sent over the connection's life, those sent again included.
    pub sent: u64,
    /// Bytes sent again because they were lost, or seemed to be.
    pub resent: u64,
    /// Bytes the client has acknowledged.
    pub acked: u64,
}

impl TcpState {
    /// Bytes written that the client hasn't acknowledged: not sent yet, or
    /// on their way, perhaps queued somewhere in the network.
    pub fn unacked(&self) -> u64 {
        (self.sent.saturating_sub(self.resent)).saturating_sub(self.acked) + u64::from(self.unsent)
    }
}

/// Reads `socket`'s state. `None` off Linux, and on kernels before 4.19,
/// which don't count bytes.
#[cfg(target_os = "linux")]
pub fn state(socket: &TcpStream) -> Option<TcpState> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::fd::AsRawFd;

    let mut info = MaybeUninit::<Info>::zeroed();
    let mut len = size_of::<Info>() as libc::socklen_t;
    // SAFETY: the kernel writes at most `len` bytes into `info`, which is
    // plain integers, and sets `len` to how many it wrote.
    let failed = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            info.as_mut_ptr().cast(),
            &mut len,
        )
    } != 0;
    if failed || (len as usize) < size_of::<Info>() {
        return None;
    }
    // SAFETY: zeroed, then filled in whole.
    let info = unsafe { info.assume_init() };
    Some(TcpState {
        rtt: Duration::from_micros(info.rtt.into()),
        unsent: info.notsent_bytes,
        sent: info.bytes_sent,
        resent: info.bytes_retrans,
        acked: info.bytes_acked,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn state(_: &TcpStream) -> Option<TcpState> {
    None
}

/// Tells the kernel `socket` is a thin stream, as a game's is: with little
/// in flight, waiting for a lost packet to be acknowledged before sending
/// it again doesn't double each time it's lost again, for the first 6
/// tries. Linux only; elsewhere it does nothing.
#[cfg(target_os = "linux")]
pub fn thin_stream(socket: &TcpStream) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let on: libc::c_int = 1;
    // SAFETY: the option takes a c_int, and gets a pointer to one and its
    // size.
    let failed = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_THIN_LINEAR_TIMEOUTS,
            (&on as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    } != 0;
    if failed {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn thin_stream(_: &TcpStream) -> std::io::Result<()> {
    Ok(())
}

/// The start of Linux's `struct tcp_info` (`linux/tcp.h`), up to the last
/// field read here; libc's copy ends before most of them.
#[cfg(target_os = "linux")]
#[repr(C)]
struct Info {
    _state_to_rcv_ssthresh: [u32; 17],
    /// `tcpi_rtt`, in µs.
    rtt: u32,
    _rttvar_to_max_pacing_rate: [u32; 12],
    bytes_acked: u64,
    _bytes_received_to_segs_in: [u32; 4],
    notsent_bytes: u32,
    _min_rtt_to_delivered_ce: [u32; 13],
    bytes_sent: u64,
    bytes_retrans: u64,
}

#[cfg(target_os = "linux")]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(offset_of!(Info, rtt) == 68);
    assert!(offset_of!(Info, bytes_acked) == 120);
    assert!(offset_of!(Info, notsent_bytes) == 144);
    assert!(offset_of!(Info, bytes_sent) == 200);
    assert!(offset_of!(Info, bytes_retrans) == 208);
    assert!(size_of::<Info>() == 216);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_kernel_counts_what_was_sent_and_what_waits() {
        use std::io::{ErrorKind, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        // Never read: once its buffers fill, what's written waits here.
        let _receiver = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        // The accepted end, as the server's are: the side that connects
        // counts its opening (its SYN) as a byte acknowledged.
        let (mut sender, _) = listener.accept().unwrap();
        sender.set_nonblocking(true).unwrap();
        let chunk = [7u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            match sender.write(&chunk) {
                Ok(n) => written += n as u64,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        let state = state(&sender).unwrap();
        assert!(state.unsent > 0, "{state:?}");
        // What was sent counts what was sent again: rare on loopback, but
        // a busy machine can need it.
        let once = state.sent - state.resent;
        assert_eq!(once + u64::from(state.unsent), written, "{state:?}");
        assert!(0 < state.acked && state.acked <= once, "{state:?}");
        assert_eq!(state.unacked(), written - state.acked, "{state:?}");
        assert!(state.rtt < Duration::from_secs(1), "{state:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn thin_streams_are_marked() {
        use std::os::fd::AsRawFd;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let read = || {
            let mut value: libc::c_int = -1;
            let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: as in `thin_stream`, the other way round.
            let got = unsafe {
                libc::getsockopt(
                    socket.as_raw_fd(),
                    libc::IPPROTO_TCP,
                    libc::TCP_THIN_LINEAR_TIMEOUTS,
                    (&mut value as *mut libc::c_int).cast(),
                    &mut len,
                )
            };
            assert_eq!(got, 0);
            value
        };
        assert_eq!(read(), 0);
        thin_stream(&socket).unwrap();
        assert_eq!(read(), 1);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_systems_say_nothing() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        assert_eq!(state(&socket), None);
    }
}
