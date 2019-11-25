use async_std::prelude::*;
use async_std::task::{Context, Poll, Waker};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

struct Zombies {
    sigid: signal_hook::SigId,
    waker: Arc<Mutex<(usize, Option<Waker>)>>,
}

impl Stream for Zombies {
    type Item = ();
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<()>> {
        let mut guard = self.waker.lock().unwrap();
        let pair = &mut guard;
        if pair.0 > 0 {
            pair.0 -= 1;
            Poll::Ready(Some(()))
        } else {
            pair.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl Drop for Zombies {
    fn drop(&mut self) {
        signal_hook::unregister(self.sigid);
    }
}

impl Zombies {
    fn new() -> Result<Self, std::io::Error> {
        let waker = Arc::new(Mutex::new((0, None)));

        let waker_clone = waker.clone();

        let handler = move || {
            let mut guard = waker_clone.lock().unwrap();
            let pair: &mut (usize, Option<Waker>) = &mut guard;
            pair.0 += 1;
            match pair.1.take() {
                None => (),
                Some(waker) => waker.wake(),
            }
        };
        let sigid = unsafe { signal_hook::register(signal_hook::SIGCHLD, handler)? };
        Ok(Zombies { waker, sigid })
    }

    async fn reap_till(mut self, till: i32) -> Result<(), Pid1Error> {
        while let Some(()) = self.next().await {
            let mut status = 0;
            loop {
                let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if pid == till {
                    return Ok(());
                } else if pid <= 0 {
                    break;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
enum Pid1Error {
    IOError(std::io::Error),
    NoCommandGiven,
    ChildPidTooBig(u32, std::num::TryFromIntError),
}

impl std::convert::From<std::io::Error> for Pid1Error {
    fn from(e: std::io::Error) -> Self {
        Pid1Error::IOError(e)
    }
}

#[async_attributes::main]
async fn main() -> Result<(), Pid1Error> {
    let (cmd, args) = get_command()?;
    let child = std::process::Command::new(cmd).args(args).spawn()?.id();

    use std::convert::TryInto;
    let child: libc::pid_t = match child.try_into() {
        Ok(x) => x,
        Err(e) => return Err(Pid1Error::ChildPidTooBig(child, e)),
    };

    let interrupt_child = move || {
        unsafe {
            libc::kill(child, libc::SIGINT); // ignoring errors
        }
    };
    let sigid: signal_hook::SigId =
        unsafe { signal_hook::register(signal_hook::SIGINT, interrupt_child)? };

    Zombies::new()?.reap_till(child).await?;

    signal_hook::unregister(sigid);
    Ok(())
}

fn get_command() -> Result<(String, Vec<String>), Pid1Error> {
    let mut args = std::env::args();
    let _me = args.next();
    match args.next() {
        None => Err(Pid1Error::NoCommandGiven),
        Some(cmd) => Ok((cmd, args.collect())),
    }
}
