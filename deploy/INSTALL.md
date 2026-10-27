# Serving SnakeTUI over SSH on Ubuntu 24.04

When you're done, anyone can type `ssh play.example.com` and land straight in
the game. There's no shell and no Linux account for players, and quitting the
game ends their connection. Your own admin login moves to port 2222.

The game is its own SSH server, so it needs port 22 for itself. Follow the
steps in order: your admin login keeps working at every point, and you check
it before going on.

You need:

- an Ubuntu 24.04 server you can `sudo` on, that you log in to with a key,
- a DNS name for it,
- access to your cloud provider's firewall, if it has one.

This sets up one server. To run several as one, set up each like this, then
follow [CLUSTER.md](CLUSTER.md).

Below, `play.example.com` is your DNS name and `you` is your admin user.

## 1. Point a DNS name at the server

Add an `A` record for `play.example.com` with the server's IPv4 address, and
an `AAAA` record if it has IPv6. The game listens on both.

## 2. Open port 2222

In your cloud provider's firewall, allow TCP 2222 from wherever you log in
from, and TCP 22 from anywhere. Port 22 is probably open already.

On the server:

```bash
sudo ufw allow 2222/tcp comment 'admin ssh'
```

This only adds a rule. If ufw is inactive (`sudo ufw status`), leave it
inactive for now; step 6 turns it on.

## 3. Move your admin SSH to port 2222

Ubuntu 24.04 starts sshd from `ssh.socket`. When systemd reloads, it reads the
`Port` from sshd's configuration and moves the socket there.

```bash
echo 'Port 2222' | sudo tee /etc/ssh/sshd_config.d/admin-port.conf
sudo sshd -t
sudo systemctl daemon-reload
sudo systemctl restart ssh.socket
```

`sshd -t` prints nothing when the configuration is fine. Your current session
stays open through the restart.

## 4. Check before going on

```bash
sudo ss -tlnp 'sport = :22 or sport = :2222'
```

You should see port 2222, and nothing on port 22.

- **Still on port 22?** `/etc/ssh/sshd_config` probably has its own `Port 22`
  line. Comment it out and repeat step 3.
- **Not on 2222 at all?** Your system lacks the generator that moves the
  socket. Run `sudo systemctl edit ssh.socket`, add the lines below, then
  repeat the last two commands of step 3.

  ```ini
  [Socket]
  ListenStream=
  ListenStream=2222
  ```

Now, **keeping this session open**, log in from a second terminal on your own
computer:

```bash
ssh -p 2222 you@play.example.com
```

Don't go on until that works. If it doesn't, fix it from the session you still
have open: the usual cause is the cloud firewall from step 2.

Once the game takes port 22, `ssh play.example.com` opens the game, even for
you. Give your admin login a name in `~/.ssh/config` on your computer:

```
Host play-admin
    HostName play.example.com
    Port 2222
    User you
```

From then on, `ssh play-admin` is your admin login.

## 5. Install the game

Download the server for your machine from the
[releases page](https://github.com/ssenerg/game/releases). Set `VERSION`
to the release you want:

```bash
VERSION=v0.2.0
NAME=snaketui-server-$VERSION-$(uname -m)-unknown-linux-gnu
cd /tmp
curl -fLO https://github.com/ssenerg/game/releases/download/$VERSION/$NAME.tar.gz
tar xzf $NAME.tar.gz
cd $NAME
```

Then install it: the program, a system user for it with no shell and no
login, the settings file, and the systemd service.

```bash
sudo install -m 0755 snaketui-server /usr/local/bin/snaketui-server
sudo useradd --system --home-dir /var/lib/snaketui --no-create-home --shell /usr/sbin/nologin snaketui
sudo install -d -m 0755 /etc/snaketui
sudo install -m 0644 server.toml.example /etc/snaketui/server.toml
sudo install -m 0644 snaketui.service /etc/systemd/system/snaketui.service
sudo systemctl daemon-reload
sudo systemctl enable --now snaketui
```

Check that it's running and has port 22:

```bash
systemctl status snaketui
sudo ss -tlnp 'sport = :22'
```

The service runs as `snaketui`. The only privilege it has is binding port 22,
and it can write to nothing but `/var/lib/snaketui`. That directory holds
everything the game keeps:

- `host_ed25519_key`: the game's own SSH host key, created on first start. It
  is separate from the machine's OpenSSH host key.
- `snaketui.db`: the accounts (usernames, password hashes, ratings and
  friends), and the nicknames, settings and records of guests who connect with
  a key.

If it doesn't start, see the log: `journalctl -u snaketui -e`.

## 6. Allow port 22 in ufw

```bash
sudo ufw allow 22/tcp comment 'snaketui'
sudo ufw status
```

If ufw was inactive, make sure the list shows the 2222 rule from step 2, then
turn it on:

```bash
sudo ufw show added
sudo ufw enable
```

Don't use `ufw limit 22`. It blocks a whole IP address after a few quick
connections, which shuts out players who share one, such as everyone on a
university or office network. The game limits connections per address itself
(`max_sessions_per_ip` in the settings).

## 7. Check

On your computer, your `known_hosts` file still has the old OpenSSH key for
`play.example.com`. The game has a different key, so remove the old entry
first:

```bash
ssh-keygen -R play.example.com
```

Then:

```bash
ssh play.example.com
```

You should see the SnakeTUI logo. Press `q`, and you're back at your own
prompt at once.

Players see the game's key fingerprint the first time they connect. To publish
it so they can check it, get it on the server with:

```bash
sudo ssh-keygen -lf /var/lib/snaketui/host_ed25519_key
```

## 8. Clean up

- **Back up the host key.** Copy `/var/lib/snaketui/host_ed25519_key`
  somewhere safe. If you rebuild the server, put it back before the first
  start; otherwise every returning player gets a "REMOTE HOST IDENTIFICATION
  HAS CHANGED" warning.
- **Back up the database.** `snaketui.db` holds every account; without it,
  players lose their accounts, ratings and friends. To copy it while the game
  is running, install `sqlite3` and use its backup command:

  ```bash
  sudo apt install sqlite3
  sudo sqlite3 /var/lib/snaketui/snaketui.db ".backup /root/snaketui-backup.db"
  ```

  Don't copy the file with `cp` while the game runs: the newest changes may
  still be in `snaketui.db-wal` next to it.
- **fail2ban.** It isn't installed on Ubuntu by default. If you installed it,
  its `sshd` jail still watches port 22. Point it at your admin port by putting
  this in `/etc/fail2ban/jail.d/sshd-port.local`, then run
  `sudo systemctl restart fail2ban`:

  ```ini
  [sshd]
  port = 2222
  ```

  Game connections don't go through OpenSSH, so fail2ban never sees or bans
  players.

## Settings

`/etc/snaketui/server.toml` lists every setting with its default: listening
address, connection limits, and timeouts. After changing it, run:

```bash
sudo systemctl restart snaketui
```

## Updating

Download the new release as in step 5, then:

```bash
sudo install -m 0755 snaketui-server /usr/local/bin/snaketui-server
sudo systemctl restart snaketui
```

Players who are connected are told the server is restarting and are logged
out. Their settings and records stay.

If the release changes `snaketui.service`, install it again too, and run
`sudo systemctl daemon-reload` before the restart.

## Troubleshooting

| You see | What to do |
|---|---|
| The log says `listening on [::]:22` and `Address already in use` | sshd still has port 22. Go back to step 4. |
| The log says it can't listen on `[::]:22` because the address family isn't supported | The machine has IPv6 turned off. Set `listen = "0.0.0.0:22"` in `/etc/snaketui/server.toml` and restart. |
| `ssh play.example.com` hangs, then times out | Port 22 is closed in ufw or in the cloud firewall (steps 2 and 6). |
| Players get "REMOTE HOST IDENTIFICATION HAS CHANGED" | The host key was replaced. Restore it from your backup and restart. |
| A player forgot their password | They can set a new one under Account → Change password from any machine that still signs them in. Otherwise the account can't be recovered, because the game has no email addresses. |
| A player sees "Too many wrong tries for that name" | 5 wrong passwords within 15 minutes lock a username for 15 minutes. An IP address gets 20 wrong tries an hour. Both clear by themselves. To unlock a name at once, run `sudo sqlite3 /var/lib/snaketui/snaketui.db "DELETE FROM name_locks WHERE name = 'alice'"` with the name in lowercase. |
| You can't reach port 2222 | Log in from your cloud provider's web console. Stop the game (`sudo systemctl stop snaketui`), delete `/etc/ssh/sshd_config.d/admin-port.conf`, then run `sudo systemctl daemon-reload` and `sudo systemctl restart ssh.socket`. sshd is back on port 22. |

For more detail in the log, run `sudo systemctl edit snaketui`, add the lines
below, and restart:

```ini
[Service]
Environment=RUST_LOG=snaketui_server=debug
```
