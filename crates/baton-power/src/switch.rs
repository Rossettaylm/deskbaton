use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use baton_core::Result;
use baton_core::ports::AwakeGuard;

/// 多个 [`AwakeGuard`] 共用的系统级开关：第一个 guard 打开，最后一个释放或到期时关闭。
pub struct SharedSwitch {
    holders: Mutex<usize>,
    on: fn() -> Result<()>,
    off: fn(),
}

impl SharedSwitch {
    pub const fn new(on: fn() -> Result<()>, off: fn()) -> Self {
        Self {
            holders: Mutex::new(0),
            on,
            off,
        }
    }

    pub fn hold(&'static self, duration: Duration) -> Result<AwakeGuard> {
        {
            let mut holders = self.lock();
            if *holders == 0 {
                (self.on)()?;
            }
            *holders += 1;
        }
        // 锁住整个释放过程：guard 被释放时要等到期线程正在做的恢复完成，进程退出前睡眠设置才确定已恢复。
        let released = Arc::new(Mutex::new(false));
        let release = {
            let released = Arc::clone(&released);
            move || {
                let mut released = released.lock().unwrap_or_else(|e| e.into_inner());
                if !*released {
                    *released = true;
                    self.release_one();
                }
            }
        };
        let (cancel, cancelled) = mpsc::channel::<()>();
        let timer_release = release.clone();
        thread::spawn(move || {
            // 到期返回 Timeout，guard 释放时 Sender 被丢弃返回 Disconnected，两种情况都要释放。
            let _ = cancelled.recv_timeout(duration);
            timer_release();
        });
        Ok(AwakeGuard::new(move || {
            release();
            drop(cancel);
        }))
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn is_on(&self) -> bool {
        *self.lock() > 0
    }

    fn release_one(&self) {
        let mut holders = self.lock();
        *holders = holders.saturating_sub(1);
        if *holders == 0 {
            (self.off)();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, usize> {
        self.holders.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use baton_core::Error;

    use super::*;

    fn wait_until(cond: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn overlapping_guards_turn_off_once_after_last() {
        static ONS: AtomicUsize = AtomicUsize::new(0);
        static OFFS: AtomicUsize = AtomicUsize::new(0);
        static SWITCH: SharedSwitch = SharedSwitch::new(
            || {
                ONS.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || {
                OFFS.fetch_add(1, Ordering::SeqCst);
            },
        );

        let a = SWITCH.hold(Duration::MAX).unwrap();
        let b = SWITCH.hold(Duration::MAX).unwrap();
        assert_eq!(ONS.load(Ordering::SeqCst), 1);
        drop(a);
        assert!(SWITCH.is_on());
        drop(b);
        assert!(!SWITCH.is_on());
        assert_eq!(OFFS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn expires_after_duration() {
        static OFFS: AtomicUsize = AtomicUsize::new(0);
        static SWITCH: SharedSwitch = SharedSwitch::new(
            || Ok(()),
            || {
                OFFS.fetch_add(1, Ordering::SeqCst);
            },
        );

        let guard = SWITCH.hold(Duration::from_millis(20)).unwrap();
        assert!(wait_until(|| !SWITCH.is_on()));
        drop(guard);
        assert_eq!(OFFS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_on_does_not_count_as_holder() {
        static SWITCH: SharedSwitch =
            SharedSwitch::new(|| Err(Error::Power("denied".into())), || {});
        assert!(SWITCH.hold(Duration::MAX).is_err());
        assert!(!SWITCH.is_on());
    }
}
