//! The SSH side of a connection: who may sign in, and the only thing they
//! may do, which is play.

use std::borrow::Cow;
use std::path::Path;
use std::time::Duration;

use actix::Addr;
use anyhow::{Context as _, Result};
use bytes::Bytes;
use russh::keys::{Algorithm, PrivateKey, PublicKey, ssh_key::LineEnding};
use russh::server::{Auth, ChannelOpenHandle, Msg, Response, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet, SshId};
use tracing::info;

use crate::identity::Player;
use crate::session::{
    Authenticated, ChannelOpened, Disconnected, Input, NEEDS_PTY, NO_COMMANDS, PtyRequested,
    Refuse, Resized, SessionActor, ShellRequested,
};

pub fn config(host_key: PrivateKey) -> russh::server::Config {
    russh::server::Config {
        server_id: SshId::Standard(Cow::Owned(format!(
            "SSH-2.0-SnakeTUI_{}",
            env!("CARGO_PKG_VERSION")
        ))),
        // Any key signs a player in; without one they're a guest. Never
        // passwords: people would type real ones.
        methods: MethodSet::from(&[MethodKind::PublicKey, MethodKind::KeyboardInteractive][..]),
        auth_rejection_time: Duration::from_secs(1),
        // OpenSSH probes with "none" first; don't make everyone wait.
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key],
        max_auth_attempts: 6,
        // Sessions time out idle players themselves; keepalives catch dead
        // connections and keep NAT and load balancer flows open.
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    }
}

/// Reads the server's host key, or creates one on first start.
pub fn load_or_create_host_key(path: &Path) -> Result<PrivateKey> {
    if path.exists() {
        return PrivateKey::read_openssh_file(path)
            .with_context(|| format!("reading host key {}", path.display()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    key.write_openssh_file(path, LineEnding::LF)
        .with_context(|| format!("writing host key {}", path.display()))?;
    info!(path = %path.display(), "created a new host key");
    Ok(key)
}

/// Handles one connection's SSH requests and forwards them to its
/// [`SessionActor`].
pub struct ConnHandler {
    session: Addr<SessionActor>,
    /// The one session channel allowed per connection.
    channel: Option<ChannelId>,
    pty: bool,
}

impl ConnHandler {
    pub fn new(session: Addr<SessionActor>) -> Self {
        Self {
            session,
            channel: None,
            pty: false,
        }
    }

    fn ours(&self, channel: ChannelId) -> bool {
        self.channel == Some(channel)
    }
}

impl Drop for ConnHandler {
    fn drop(&mut self) {
        self.session.do_send(Disconnected);
    }
}

impl russh::server::Handler for ConnHandler {
    type Error = anyhow::Error;

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth> {
        self.session
            .do_send(Authenticated(Player::with_key(user, key)));
        Ok(Auth::Accept)
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        _response: Option<Response<'a>>,
    ) -> Result<Auth> {
        self.session.do_send(Authenticated(Player::guest(user)));
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<()> {
        // Dropping `reply` turns away any channel after the first.
        if self.channel.is_none() {
            reply.accept().await;
            self.channel = Some(channel.id());
            self.session.do_send(ChannelOpened {
                channel: channel.id(),
                handle: session.handle(),
            });
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<()> {
        if self.ours(channel) {
            self.pty = true;
            self.session.do_send(PtyRequested { cols, rows });
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        if !self.ours(channel) {
            return Ok(session.channel_failure(channel)?);
        }
        session.channel_success(channel)?;
        if self.pty {
            self.session.do_send(ShellRequested);
        } else {
            self.session.do_send(Refuse(NEEDS_PTY));
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _command: &[u8],
        session: &mut Session,
    ) -> Result<()> {
        // Accept, so the client prints the explanation, then close.
        session.channel_success(channel)?;
        self.session.do_send(Refuse(NO_COMMANDS));
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        session: &mut Session,
    ) -> Result<()> {
        session.channel_failure(channel)?;
        self.session.do_send(Refuse(NO_COMMANDS));
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        _value: &str,
        session: &mut Session,
    ) -> Result<()> {
        Ok(session.channel_failure(channel)?)
    }

    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _single_connection: bool,
        _x11_auth_protocol: &str,
        _x11_auth_cookie: &str,
        _x11_screen_number: u32,
        session: &mut Session,
    ) -> Result<()> {
        Ok(session.channel_failure(channel)?)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<()> {
        if self.ours(channel) {
            let _ = self.session.try_send(Resized { cols, rows });
        }
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _: &mut Session) -> Result<()> {
        // A full mailbox drops keys: a cheap limit on how fast anyone can type.
        if self.ours(channel) {
            let _ = self.session.try_send(Input(Bytes::copy_from_slice(data)));
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _: &mut Session) -> Result<()> {
        if self.ours(channel) {
            self.session.do_send(Disconnected);
        }
        Ok(())
    }

    async fn channel_close(&mut self, channel: ChannelId, _: &mut Session) -> Result<()> {
        if self.ours(channel) {
            self.session.do_send(Disconnected);
        }
        Ok(())
    }
}
