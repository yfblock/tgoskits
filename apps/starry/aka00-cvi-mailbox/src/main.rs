//! mbmon — 轮询 `/dev/cvi-mailbox`，打印小核（C906L）上报的抓帧状态。
//!
//! 用法：mbmon [/dev/cvi-mailbox]
//! 每秒读一次 16 字节 Mailbox（magic/frame_count/last_size/flags）并打印。
//! magic=0xC906C906 正常；0xFFFFFFFF 小核 panic；其它→小核未运行。

use std::fs::File;
use std::io::Read;
use std::thread::sleep;
use std::time::Duration;

const MAGIC: u32 = 0xC906_C906;
const PANIC_MAGIC: u32 = 0xFFFF_FFFF;

fn main() {
    let dev = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/cvi-mailbox".into());
    let mut f = File::open(&dev).expect("open mailbox device");

    let mut buf = [0u8; 16];
    loop {
        // 设备每次 read 返回当前邮箱（16B）；偏移语义不影响。
        match f.read(&mut buf) {
            Ok(n) if n >= 16 => {
                let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                let frame_count = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
                let last_size = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
                let flags = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
                match magic {
                    MAGIC => println!(
                        "c906l: frames={} last={}B SOI={} EOI={}",
                        frame_count,
                        last_size,
                        flags & 1 != 0,
                        flags & 2 != 0,
                    ),
                    PANIC_MAGIC => println!("c906l: PANIC (small core panicked)"),
                    _ => println!(
                        "c906l: not running? magic={:#010x} (expect {:#010x})",
                        magic, MAGIC
                    ),
                }
            }
            Ok(n) => eprintln!("short read: {} bytes", n),
            Err(e) => eprintln!("read err: {}", e),
        }
        sleep(Duration::from_secs(1));
    }
}
