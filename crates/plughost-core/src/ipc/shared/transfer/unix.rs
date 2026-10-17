use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixDatagram;
use std::process::{Child, Stdio};

use rustix::net::{
    self, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};

use super::super::{
    Activity, Descriptor, SharedAudio,
    doorbell::Doorbell,
    errors::{TRANSFER, invalid},
    realtime::{Ring, RingDescriptor},
};

/// Tags the activity datagram; shared audio generations start at 1.
const ACTIVITY_TAG: u64 = 0;

pub struct Sender {
    socket: UnixDatagram,
}
pub struct Receiver {
    socket: UnixDatagram,
}

impl Sender {
    /// The returned stdio must be installed as the helper's stdin before spawning it.
    pub fn new() -> io::Result<(Self, Stdio)> {
        let (socket, child) = UnixDatagram::pair()?;
        Ok((Self { socket }, Stdio::from(OwnedFd::from(child))))
    }

    /// Sends ownership of a duplicate file descriptor. Nonblocking: a stalled peer cannot hold up
    /// the application here. At most one preparation may be outstanding on this connection.
    pub fn send(&self, memory: &SharedAudio, _child: &Child) -> io::Result<u64> {
        self.send_fd(memory.memory.handle(), memory.descriptor.generation)
    }

    /// Sends the activity mapping the same way, for the load request that carries its token.
    pub fn send_activity(&self, activity: &Activity, _child: &Child) -> io::Result<u64> {
        self.send_fd(activity.memory.handle(), ACTIVITY_TAG)
    }

    /// Sends a realtime ring's control mapping and then its lanes, and returns their tokens.
    pub fn send_ring(&self, ring: &Ring, _child: &Child) -> io::Result<Vec<u64>> {
        let descriptor = ring.descriptor();
        let mut tokens = vec![self.send_fd(ring.control_memory().handle(), descriptor.control)?];
        for lane in ring.lane_audio() {
            tokens.push(self.send_fd(lane.memory.handle(), lane.descriptor.generation)?);
        }
        Ok(tokens)
    }

    fn send_fd(&self, fd: &OwnedFd, tag: u64) -> io::Result<u64> {
        let payload = tag.to_le_bytes();
        let fds = [fd.as_fd()];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        if !control.push(SendAncillaryMessage::ScmRights(&fds)) {
            return Err(invalid(TRANSFER));
        }
        let sent = net::sendmsg(
            &self.socket,
            &[IoSlice::new(&payload)],
            &mut control,
            SendFlags::DONTWAIT,
        )?;
        if sent != payload.len() {
            return Err(invalid(TRANSFER));
        }
        Ok(tag)
    }
}

impl Receiver {
    /// Takes the inherited bootstrap socket, then restores null stdin before loading plugins.
    /// Call once during helper startup, before any plugin or stdin-reading thread is started.
    pub fn take_from_stdin() -> io::Result<Self> {
        let fd = io::stdin().as_fd().try_clone_to_owned()?;
        rustix::stdio::dup2_stdin(std::fs::File::open("/dev/null")?)?;
        Ok(Self {
            socket: UnixDatagram::from(fd),
        })
    }

    /// Receives exactly one transferred mapping. The parent sends the handle before its prepare
    /// notification, so a missing datagram is a protocol error rather than an unbounded wait.
    ///
    /// # Safety
    /// The transfer must come from the parent using `Sender::send`, and the parent must never
    /// access the shared memory other than through `SharedAudio` while it is mapped.
    /// `descriptor` must be the exact descriptor of the transferred mapping, including precision.
    /// Each transfer is received once. Windows additionally requires a live, uniquely owned handle.
    pub unsafe fn receive(&self, descriptor: Descriptor, token: u64) -> io::Result<SharedAudio> {
        if token != descriptor.generation {
            return Err(invalid(TRANSFER));
        }
        // SAFETY: the parent sent the shared memory of its `SharedAudio` for `descriptor`.
        unsafe { SharedAudio::open(self.receive_fd(token)?, descriptor) }
    }

    /// Receives the activity mapping sent before the load request carrying `token`.
    ///
    /// # Safety
    /// As for [`Receiver::receive`], with a transfer from `Sender::send_activity`.
    pub unsafe fn receive_activity(&self, token: u64) -> io::Result<Activity> {
        if token != ACTIVITY_TAG {
            return Err(invalid(TRANSFER));
        }
        // SAFETY: the parent sent the shared memory of its `Activity`.
        unsafe { Activity::open(self.receive_fd(token)?) }
    }

    /// Receives the realtime ring sent with `tokens` for the request carrying `descriptor`.
    ///
    /// # Safety
    /// As for [`Receiver::receive`], with transfers from `Sender::send_ring`.
    pub unsafe fn receive_ring(
        &self,
        descriptor: RingDescriptor,
        tokens: &[u64],
    ) -> io::Result<Ring> {
        let [control, lanes @ ..] = tokens else {
            return Err(invalid(TRANSFER));
        };
        if *control != descriptor.control
            || lanes.len() != descriptor.lanes
            || lanes
                .iter()
                .enumerate()
                .any(|(index, &token)| token != descriptor.first_lane + index as u64)
        {
            return Err(invalid(TRANSFER));
        }
        let control = self.receive_fd(*control)?;
        let lanes = lanes
            .iter()
            .map(|&token| self.receive_fd(token))
            .collect::<io::Result<_>>()?;
        // SAFETY: the parent sent its ring's mappings for `descriptor`, in order.
        unsafe { Ring::open(descriptor, control, lanes, Doorbell::new()?) }
    }

    fn receive_fd(&self, tag: u64) -> io::Result<OwnedFd> {
        let mut payload = [0; 8];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let received = net::recvmsg(
            &self.socket,
            &mut [IoSliceMut::new(&mut payload)],
            &mut control,
            RecvFlags::DONTWAIT,
        )?;
        let mut received_fd = None;
        for message in control.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = message {
                for fd in fds {
                    if received_fd.is_some() {
                        return Err(invalid(TRANSFER));
                    }
                    rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
                    received_fd = Some(fd);
                }
            }
        }
        if received.bytes != payload.len()
            || received
                .flags
                .intersects(net::ReturnFlags::TRUNC | net::ReturnFlags::CTRUNC)
            || u64::from_le_bytes(payload) != tag
        {
            return Err(invalid(TRANSFER));
        }
        received_fd.ok_or_else(|| invalid(TRANSFER))
    }
}
