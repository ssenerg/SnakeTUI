# Running SnakeTUI on several servers

One server, set up with [INSTALL.md](INSTALL.md), holds a couple of thousand
players. For more players, or to serve players in several parts of the world,
run several servers as one cluster. Players on any of them:

- share private rooms by code,
- see their friends online and invite them,
- wait in one quick-play queue, matched by rating and region.

A cluster adds a few pieces next to the game servers:

| Piece | What it does |
|---|---|
| **Postgres** 16 or newer | Accounts, friends, ratings, room codes and guessing limits, for every server. |
| **NATS** 2.10 or newer | Messages between servers: who's alive, players in rooms on another server, friends and invites. Plain NATS is enough; JetStream isn't used. |
| **snaketui-matchmaker** | The quick-play queue. Exactly one runs per cluster. |
| **HAProxy** | Optional. Spreads a region's players over its servers and keeps them away from a server that is stopping. |

Ubuntu 24.04's own `postgresql`, `nats-server` and `haproxy` packages work.

## Try it on one machine first

[deploy/cluster/](https://github.com/ssenerg/SnakeTUI/tree/main/deploy/cluster)
in the source has a docker-compose file with all of it: two game servers
behind HAProxy, Postgres, NATS and the matchmaker. With Docker installed,
from that directory:

```bash
docker compose up --build -d
```

```bash
ssh -p 2222 localhost
```

Its `server.toml` and `haproxy.cfg` are working examples of the files below.

## 1. Plan the machines

This guide uses one region, `eu`, with these machines on a private network,
10.0.0.0/24. Use your own names and addresses.

| Machine | Private address | Runs |
|---|---|---|
| `db` | 10.0.0.5 | Postgres, NATS, snaketui-matchmaker |
| `lb-eu` | 10.0.0.10, and a public address | HAProxy |
| `game-eu-1` | 10.0.0.11 | snaketui-server |
| `game-eu-2` | 10.0.0.12 | snaketui-server |

Every machine runs Ubuntu 24.04.

**Another region** gets its own load balancer and game servers, with
`region` set to its own name (step 4). They use the same `db`, so they need a
private link to it, such as your provider's network peering or a WireGuard
VPN. NATS here has a password but no encryption: don't let it cross the
internet in the open.

**Passwords.** Steps 2 and 3 each need a password. Passwords go inside URLs
in the settings, so make them from letters and digits only:

```bash
openssl rand -hex 24
```

## 2. Postgres

On `db`:

```bash
sudo apt install postgresql
sudo -u postgres createuser --pwprompt snaketui
sudo -u postgres createdb --owner snaketui snaketui
```

`createuser` asks for the database password. Then let the game servers in
from the private network:

```bash
echo "listen_addresses = 'localhost,10.0.0.5'" | sudo tee /etc/postgresql/16/main/conf.d/snaketui.conf
echo "host snaketui snaketui 10.0.0.0/24 scram-sha-256" | sudo tee -a /etc/postgresql/16/main/pg_hba.conf
sudo systemctl restart postgresql
sudo ufw allow from 10.0.0.0/24 to any port 5432 proto tcp comment 'postgres'
```

These ufw rules only count once ufw is on. If you turn it on for `db`, allow
your own login first (`sudo ufw allow OpenSSH`), or you lock yourself out.

The game servers create their tables on first start.

Each game server keeps up to 16 connections to Postgres. Postgres allows
100 by default, which is enough for 5 servers. For more, raise
`max_connections` in the same `conf.d/snaketui.conf` to 16 for each server,
plus 20, and restart Postgres.

## 3. NATS

On `db`:

```bash
sudo apt install nats-server
```

Ubuntu's NATS listens only on 127.0.0.1. Replace `/etc/nats-server.conf`
with this, using your NATS password:

```
# The private network only: NATS here has no TLS.
host: 10.0.0.5
port: 4222

authorization {
  user: snaketui
  password: "NATS_PASSWORD"
}
```

Only the service should read the password. Then check the file and start
NATS:

```bash
sudo chown root:nats /etc/nats-server.conf
sudo chmod 0640 /etc/nats-server.conf
sudo nats-server -c /etc/nats-server.conf -t
sudo systemctl enable nats-server
sudo systemctl restart nats-server
sudo ufw allow from 10.0.0.0/24 to any port 4222 proto tcp comment 'nats'
```

## 4. The game servers

On each game server, follow [INSTALL.md](INSTALL.md) steps 2 to 6: they move
your admin login to port 2222 and install the game. Point DNS at the load
balancer instead (step 8 here), not at each server.

Then replace `/etc/snaketui/server.toml` with these settings. Each server
has its own `admin_listen` address. Everything else is the same on every
server of the region.

```toml
listen = "[::]:22"
# Health checks for HAProxy, and metrics for Prometheus.
admin_listen = "10.0.0.11:9022"
database = "postgres://snaketui:DB_PASSWORD@10.0.0.5/snaketui"
# HAProxy says who each player is.
trusted_proxies = ["10.0.0.10"]

[cluster]
nats = "nats://snaketui:NATS_PASSWORD@10.0.0.5:4222"
name = "snaketui"
region = "eu"
```

[server.toml.example](server.toml.example) explains every setting. `name`
must be the same for every server of the cluster. `region` is a name of your
choosing, made of letters, digits, `_` and `-`.

The file now holds both passwords, so only the service should read it:

```bash
sudo chown root:snaketui /etc/snaketui/server.toml
sudo chmod 0640 /etc/snaketui/server.toml
sudo ufw allow from 10.0.0.0/24 to any port 9022 proto tcp comment 'snaketui health'
sudo systemctl restart snaketui
journalctl -u snaketui -e
```

The log says `joined the cluster`, with `peers` counting the other servers it
heard from. Once a second server joins, the first one logs `a server joined`.

Behind HAProxy, the game servers only need port 22 open to it. To keep
everyone else out, replace INSTALL.md's rule:

```bash
sudo ufw delete allow 22/tcp
sudo ufw allow from 10.0.0.10 to any port 22 proto tcp comment 'snaketui via haproxy'
```

## 5. One host key for every server

Players check the game's host key, so every server must show the same one.
Otherwise a player who lands on another server gets a "REMOTE HOST
IDENTIFICATION HAS CHANGED" warning.

Copy `game-eu-1`'s key to every other server. On `game-eu-1`, put a copy
in your home directory:

```bash
sudo install -m 0600 -o $USER /var/lib/snaketui/host_ed25519_key ~/host_ed25519_key
```

From your own computer, copy it across. This uses an admin login for each
server in `~/.ssh/config`, as in INSTALL.md step 4, here called `eu-1-admin`
and `eu-2-admin`:

```bash
scp -3 eu-1-admin:host_ed25519_key eu-2-admin:
ssh eu-1-admin rm host_ed25519_key
```

On each other server:

```bash
sudo install -m 0600 -o snaketui -g snaketui ~/host_ed25519_key /var/lib/snaketui/host_ed25519_key
rm ~/host_ed25519_key
sudo systemctl restart snaketui
```

Check that every server prints the same fingerprint:

```bash
sudo -u snaketui snaketui-server --config /etc/snaketui/server.toml --make-host-key
```

## 6. The matchmaker

Quick play needs exactly one `snaketui-matchmaker` for the whole cluster. It
only talks to NATS, so it can run anywhere; here it runs on `db`.

Download the server release on `db` as in INSTALL.md step 5. In the
release's directory, write a `server.toml` with the same `database` and
`[cluster]` table as the game servers'. The matchmaker reads only those.
Then, from that directory:

```bash
sudo install -m 0755 snaketui-matchmaker /usr/local/bin/snaketui-matchmaker
sudo useradd --system --home-dir /var/lib/snaketui --no-create-home --shell /usr/sbin/nologin snaketui
sudo install -d -m 0755 /etc/snaketui
sudo install -m 0640 -g snaketui server.toml /etc/snaketui/server.toml
sudo install -m 0644 snaketui-matchmaker.service /etc/systemd/system/snaketui-matchmaker.service
sudo systemctl daemon-reload
sudo systemctl enable --now snaketui-matchmaker
journalctl -u snaketui-matchmaker -e
```

The log says `matchmaking for the cluster`.

While the matchmaker is stopped, quick play waits, and everything else keeps
working. When it's back, the servers send it their waiting players again, so
nobody loses their place. Don't run two: they would each match the same
players.

## 7. HAProxy

On `lb-eu`, first move your admin login to port 2222, as in INSTALL.md steps
2 to 4, so HAProxy can take port 22. Then:

```bash
sudo apt install haproxy
```

Replace `/etc/haproxy/haproxy.cfg` with this, with one `server` line for
each game server of the region:

```
global
    log /dev/log local0
    chroot /var/lib/haproxy
    stats socket /run/haproxy/admin.sock mode 660 level admin
    user haproxy
    group haproxy
    daemon
    maxconn 20000

defaults
    log global
    mode tcp
    option tcplog
    timeout connect 5s
    # Players stay connected as long as they play.
    timeout client 2h
    timeout server 2h
    # A stopping server refuses connections: try another.
    retries 3
    option redispatch

frontend players
    bind [::]:22 v4v6
    default_backend game

backend game
    # Each player goes to the healthy server with the fewest players.
    balance leastconn
    # A server answers 503 here while it stops, and gets no new players.
    option httpchk GET /healthz
    # The PROXY header tells the server who the player really is.
    default-server send-proxy-v2 check port 9022 inter 2s fall 2 rise 2
    server game-eu-1 10.0.0.11:22
    server game-eu-2 10.0.0.12:22
```

Check it, apply it, and let players in. If ufw is off on `lb-eu`, turn it
on afterwards as in INSTALL.md step 6.

```bash
sudo haproxy -c -f /etc/haproxy/haproxy.cfg
sudo systemctl reload haproxy
sudo ufw allow 22/tcp comment 'snaketui'
```

If a game server fails its health checks, `/var/log/haproxy.log` says it is
DOWN.

**Without HAProxy:** list every game server's public address under the
region's DNS name instead, and leave `trusted_proxies` out. `ssh` tries the
addresses in turn, so a stopping server's players reach another. Players
are spread less evenly, though, and each game server needs port 22 open to
everyone.

## 8. DNS

Give each region a name pointing at its load balancer's public address, for
example `eu.play.example.com`. With one region, `play.example.com` is
enough.

With several regions, a GeoDNS service (such as Route 53's latency or
geolocation records, or Cloudflare Load Balancing) can answer
`play.example.com` with the nearest region. Every server has the same host
key, so players see the same fingerprint under every name.

## 9. Check

On `lb-eu`, each game server says it's serving:

```bash
curl -s http://10.0.0.11:9022/healthz
```

On your own computer:

```bash
ssh-keygen -R eu.play.example.com
ssh eu.play.example.com
```

Connect from two terminals at once. HAProxy usually puts them on different
servers, and they can still play together: create a private room in one and join it
with its code in the other, or choose Quick play in both.
`snaketui_players` in each server's metrics shows where they landed:

```bash
curl -s http://10.0.0.11:9022/metrics | grep '^snaketui_players'
```

## Restarting and adding servers

`sudo systemctl restart snaketui` on a game server drains it first:

- It takes no new players at once. HAProxy tries their connections on
  another server, and stops trying this one within 4 seconds, when its
  health check fails.
- Players in a multiplayer round finish it, for up to `drain_timeout_secs`
  (200 seconds).
- Everyone else is told the server is restarting. When they reconnect,
  HAProxy puts them on another server.

The command returns once that's done.

To add a server, set it up as in steps 4 and 5, add its `server` line to the
region's `haproxy.cfg`, and run `sudo systemctl reload haproxy`. A reload
keeps everyone connected. A `restart` of HAProxy drops every player in the
region.

## Updating

Download the new release on every machine, as in INSTALL.md step 5. Then,
one game server at a time:

```bash
sudo install -m 0755 snaketui-server /usr/local/bin/snaketui-server
sudo systemctl restart snaketui
```

And on `db`:

```bash
sudo install -m 0755 snaketui-matchmaker /usr/local/bin/snaketui-matchmaker
sudo systemctl restart snaketui-matchmaker
```

If the release changes how servers talk to each other, servers on different
releases ignore each other's messages. Until all of them are updated, they
don't share rooms or friends, and quick play may wait. Update them all, one
after another, without long pauses in between.

If the release changes `snaketui.service` or `snaketui-matchmaker.service`,
install them again too, and run `sudo systemctl daemon-reload` before the
restart.

## Metrics

Each game server answers `http://ADMIN_LISTEN/metrics` in Prometheus's
format:

| Metric | What it counts |
|---|---|
| `snaketui_players` | Players connected to this server. |
| `snaketui_rooms` | Rooms running on this server. |
| `snaketui_queue_waiting` | Players waiting for a quick-play match, in the whole cluster. |
| `snaketui_draining` | 1 while the server is stopping. |
| `snaketui_cluster_peers` | Other servers it heard from in the last 2 seconds. |
| `snaketui_tick_lag_seconds` | A histogram of how late room ticks ran. Below 0.005 s at the 99th percentile is normal. |

For Prometheus on the private network:

```yaml
scrape_configs:
  - job_name: snaketui
    static_configs:
      - targets: ["10.0.0.11:9022", "10.0.0.12:9022"]
```

## Backups

- **Postgres** holds every account. Back it up from `db`:

  ```bash
  sudo -u postgres pg_dump -Fc snaketui > snaketui-$(date +%F).dump
  ```

  `pg_restore` puts it back.
- **The host key.** Keep one copy of it, as in INSTALL.md step 8. It's the
  same on every server.

A server's SQLite file isn't used in a cluster. Accounts from a server that
used to play on its own stay in its `snaketui.db`: there's no tool to move
them into Postgres yet.

## Troubleshooting

| You see | What to do |
|---|---|
| The log says `connecting to NATS` and `authorization violation` | The user or password in `cluster.nats` differs from `/etc/nats-server.conf`. |
| The log says `connecting to NATS`, with any other error | NATS isn't listening on the private address (step 3), or ufw on `db` blocks port 4222. |
| The log says `password authentication failed` or `no pg_hba.conf entry` | Check the password in `database`, and the `pg_hba.conf` line from step 2. |
| `snaketui_cluster_peers` is 0, and room codes from one server aren't found on another | The servers use a different `cluster.name` or NATS, or run different releases. |
| Quick play never finds a match, but private rooms work | The matchmaker isn't running: `systemctl status snaketui-matchmaker` on `db`. |
| Players through HAProxy are cut off at once | A game server only reads PROXY headers from `trusted_proxies`, and needs one from each of them. Check that it lists the load balancer's private address, and that `haproxy.cfg` has `send-proxy-v2`. With `RUST_LOG=snaketui_server=debug` (INSTALL.md), the log says `a bad PROXY header` or `no PROXY header in time`. |
| Players get "REMOTE HOST IDENTIFICATION HAS CHANGED" now and then | The servers have different host keys. Repeat step 5. |
| A player sees "Too many wrong tries for that name" | The limits are as in INSTALL.md, shared by every server. To unlock a name at once, run `sudo -u postgres psql snaketui -c "DELETE FROM name_locks WHERE name = 'alice'"` on `db`, with the name in lowercase. |
