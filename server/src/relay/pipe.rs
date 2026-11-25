// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
// Device Faker: https://github.com/Seyud/device_faker

//! 数据面共用的最小异步 IO 与双向泵（HTTP 与 SOCKS5 listener 共用）。
//!
//! 只用 `AsyncRead`/`AsyncWrite` 的核心 trait（不依赖 tokio 的 io-util），
//! 三条硬要求：
//!
//! 1. **两个方向都受 idle 约束**：读等 idle 就停；写也一样（对端不背压时不能无限阻塞，
//!    否则空闲超时形同虚设）；
//! 2. 短写重试：`poll_write` 返回 `Ok(n)` 就推进剩余数据；
//! 3. 半关闭：一侧 EOF/超时后对写端 `poll_shutdown`（同样带 idle 上限，不无限等）。

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

/// 单次搬运的缓冲大小。
pub const PUMP_BUFFER: usize = 16 * 1024;

/// 读一次（等价于 `AsyncReadExt::read`）。
pub async fn read_some<R>(reader: &mut R, buffer: &mut [u8]) -> io::Result<usize>
where
    R: AsyncRead + Unpin,
{
    let mut read_buf = tokio::io::ReadBuf::new(buffer);
    std::future::poll_fn(|context: &mut Context<'_>| {
        Pin::new(&mut *reader).poll_read(context, &mut read_buf)
    })
    .await?;
    Ok(read_buf.filled().len())
}

/// 写满（等价于 `AsyncWriteExt::write_all`）。
pub async fn write_all<W>(writer: &mut W, data: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut remaining = data;
    std::future::poll_fn(|context: &mut Context<'_>| {
        while !remaining.is_empty() {
            match Pin::new(&mut *writer).poll_write(context, remaining) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero)));
                }
                Poll::Ready(Ok(written)) => remaining = &remaining[written..],
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Ok(()))
    })
    .await
}

/// 半关闭写端（等价于 `AsyncWriteExt::shutdown`）。
pub async fn shutdown_write<W>(writer: &mut W) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    std::future::poll_fn(|context: &mut Context<'_>| Pin::new(&mut *writer).poll_shutdown(context))
        .await
}

/// 当前时刻（毫秒，仅用于 idle 记账）。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 单向泵：读到 EOF / 出错 / 双向都静默超过 `idle` 就收尾并对对侧 shutdown(WR)。
///
/// **写也带 idle**（F1）：对端不读时 `write_all` 会在 `PENDING` 上一直等，
/// 只给读加超时等于空闲超时失效。这里两个方向都掐。
pub async fn pump<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle: Duration,
    last: Arc<AtomicU64>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; PUMP_BUFFER];
    loop {
        match timeout(idle, read_some(reader, &mut buffer)).await {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(read)) => {
                last.store(now_millis(), Ordering::Relaxed);
                match timeout(idle, write_all(writer, &buffer[..read])).await {
                    Ok(Ok(())) => {}
                    // 写不出来（对端不背压 / 出错 / 超时）→ 掐断，别把连接挂死
                    _ => break,
                }
            }
            Err(_) => {
                let quiet = now_millis().saturating_sub(last.load(Ordering::Relaxed));
                if quiet >= idle.as_millis() as u64 {
                    break;
                }
            }
        }
    }
    // 半关闭也带上限：对端不收 FIN 时不能把收尾拖成无限等待。
    let _ = timeout(idle, shutdown_write(writer)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};

    async fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let client = TcpStream::connect(address).await.expect("connect");
        let (server, _) = listener.accept().await.expect("accept");
        (client, server)
    }

    #[tokio::test]
    async fn pump_copies_and_closes_on_eof() {
        let (mut client, server) = tcp_pair().await;
        let (mut server_read, mut server_write) = server.into_split();
        let last = Arc::new(AtomicU64::new(0));
        let idle = Duration::from_millis(300);
        let task = tokio::spawn(async move {
            let _ = pump(&mut server_read, &mut server_write, idle, last).await;
        });
        write_all(&mut client, b"ping").await.expect("write");
        let mut buffer = [0u8; 4];
        assert_eq!(read_some(&mut client, &mut buffer).await.expect("read"), 4);
        assert_eq!(&buffer, b"ping");
        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }

    /// 每次只写 1 字节的 Writer：验证短写重试。
    struct OneByteWriter {
        written: Vec<u8>,
    }

    impl AsyncWrite for OneByteWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            if buffer.is_empty() {
                return Poll::Ready(Ok(0));
            }
            self.written.push(buffer[0]);
            Poll::Ready(Ok(1))
        }
        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn write_all_retries_short_writes() {
        let mut writer = OneByteWriter {
            written: Vec::new(),
        };
        write_all(&mut writer, b"hello").await.expect("write");
        assert_eq!(writer.written, b"hello", "短写必须循环到写完");
    }

    /// 永远返回 Ok(0) 的 Writer：必须映射成 WriteZero，而不是死循环。
    struct ZeroWriter;

    impl AsyncWrite for ZeroWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(0))
        }
        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn write_all_maps_zero_write_to_error() {
        let mut writer = ZeroWriter;
        let error = write_all(&mut writer, b"x")
            .await
            .expect_err("Ok(0) 必须是错");
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }

    /// 先 Pending（唤醒自己）再给数据：验证 poll 状态机正确。
    struct PendingThenData {
        data: Vec<u8>,
        offset: usize,
        polled: bool,
    }

    impl AsyncRead for PendingThenData {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if !self.polled {
                self.polled = true;
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            let remaining = self.data.len() - self.offset;
            let take = remaining.min(buffer.remaining());
            let start = self.offset;
            buffer.put_slice(&self.data[start..start + take]);
            self.offset += take;
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn read_some_handles_pending_then_data() {
        let mut reader = PendingThenData {
            data: b"late".to_vec(),
            offset: 0,
            polled: false,
        };
        let mut buffer = [0u8; 8];
        assert_eq!(read_some(&mut reader, &mut buffer).await.expect("read"), 4);
        assert_eq!(&buffer[..4], b"late");
    }

    /// 记录是否被半关闭的 Writer。
    #[derive(Default)]
    struct RecordingWriter {
        shutdown: bool,
    }

    impl AsyncWrite for RecordingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buffer.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            self.shutdown = true;
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn pump_shuts_down_the_writer_after_eof() {
        // 半关闭：读侧 EOF 之后必须对写端 shutdown(WR)，另一方向才能继续到收尾。
        let (mut feed, mut reader_half) = tcp_pair().await;
        write_all(&mut feed, b"tail").await.expect("feed");
        drop(feed);
        let last = Arc::new(AtomicU64::new(0));
        let mut writer = RecordingWriter::default();
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            pump(
                &mut reader_half,
                &mut writer,
                Duration::from_millis(200),
                last,
            ),
        )
        .await;
        assert!(outcome.is_ok(), "EOF 后 pump 必须收尾");
        assert!(writer.shutdown, "EOF 后必须对写端 shutdown（半关闭语义）");
    }

    /// 写端永远 Pending（对端零窗口的等价物）：没有写超时就会永久阻塞。
    struct PendingWriter;

    impl AsyncWrite for PendingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn pump_stops_when_the_writer_never_drains() {
        // 读侧有数据、写侧永远 Pending：F1 要求靠写侧 idle 收尾，
        // 否则连接永挂、limiter/ActiveGuard 都释放不了。
        let (mut feed, mut reader_half) = tcp_pair().await;
        write_all(&mut feed, b"payload-to-a-stalled-writer")
            .await
            .expect("feed");
        let idle = Duration::from_millis(150);
        let last = Arc::new(AtomicU64::new(0));
        let mut writer = PendingWriter;
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            pump(&mut reader_half, &mut writer, idle, last),
        )
        .await;
        assert!(
            outcome.is_ok(),
            "写端不背压时 pump 必须靠 idle 收尾，不能挂死"
        );
        assert!(
            started.elapsed() >= idle && started.elapsed() < Duration::from_secs(3),
            "收尾必须发生在 idle 之后且不无限等待，实际 {:?}",
            started.elapsed()
        );
        drop(feed);
    }
}
