#!/usr/bin/env python3
"""Apply the name-based exclusions and write the ordered candidate list.

Excluded:
  * rule_excluded from enumerate.py (license / size / template);
  * DecBench's 40 projects and a fixed list of well-known projects, matched on
    the repository name (case-insensitive, '_'/'.' read as '-') equal to a
    listed name or starting with "<name>-" (forks/variants such as zlib-ng);
  * the same names as the repository OWNER (official orgs, e.g. curl/*, lua/*).
Everything else keeps the sha256 order from enumerate.py.
usage: exclude.py candidates_raw.json candidates.json excluded.json
"""
import json, re, sys

DECBENCH = """base-passwd bash betaflight bzip2 chibios cleanflight coreutils crazyflie cronie dash
dexter diffutils dpkg e2fsprogs findutils freertos gnutls grep gzip iproute2 kmod libacl libbsd
libedit libexpat expat libopencm3 libselinux minipig mirai mydoom nuttx openssh-portable openssh
riot-os riot rsyslog shadow sysvinit tar u-boot x0r-usb zlib""".split()
FAMOUS = """sqlite curl busybox openssl lua luajit git vim neovim nginx redis valkey ffmpeg libpng jq
zstd lz4 linux kernel glibc musl gcc llvm binutils gdb python cpython php ruby perl tcl postgres
postgresql mysql mariadb memcached haproxy varnish apache httpd lighttpd openvpn wireguard
libuv libevent libev libcurl libxml2 libjpeg libjpeg-turbo libtiff libwebp giflib freetype
harfbuzz cairo pixman glib gtk systemd dbus util-linux iptables nftables tcpdump libpcap
wireshark nmap openldap cyrus-sasl krb5 libssh libssh2 mbedtls wolfssl libsodium boringssl
libressl gnupg libgcrypt nettle gmp mpfr readline ncurses less sed gawk make m4 bison flex
autoconf automake libtool pkg-config cmake meson ninja tmux screen htop zsh fish emacs nano
mutt git-lfs rsync openssh wget2 wget aria2 libarchive xz brotli snappy lzo lzma 7zip p7zip
unrar zip unzip libzip cjson json-c jansson yajl libyaml pcre pcre2 oniguruma re2c sds hiredis
mongoose civetweb lwip contiki zephyr mbed micropython circuitpython esp-idf arduino qemu
bochs dosbox mame retroarch doom quake quake2 quake3 ioq3 openttd sdl sdl2 raylib glfw
mesa xorg wayland weston libinput alsa pulseaudio pipewire jack mpv vlc gstreamer x264 x265
opus libvorbis libogg flac lame sox imagemagick graphicsmagick gimp inkscape blender
tinycc tcc chibicc 8cc lcc cproc qbe stb nuklear raylib cosmopolitan""".split()
NAMES = {n.lower() for n in DECBENCH + FAMOUS}


def norm(s):
    return re.sub(r"[_.]", "-", s.lower())


def hit(name):
    n = norm(name)
    for x in NAMES:
        if n == x or n.startswith(x + "-"):
            return x
    return None


raw = json.load(open(sys.argv[1]))
keep, drop = [], []
for r in raw["repos"]:
    owner, name = r["full_name"].split("/", 1)
    why = r["rule_excluded"]
    if not why and (h := hit(name)):
        why = f"name:{h}"
    if not why and (h := hit(owner)) and norm(owner) == h:
        why = f"owner:{h}"
    (drop if why else keep).append({**r, "excluded": why} if why else r)
json.dump({"snapshot": raw["snapshot"], "count": len(keep), "repos": keep}, open(sys.argv[2], "w"), indent=1)
json.dump({"count": len(drop), "repos": [{"full_name": r["full_name"], "stars": r["stars"],
          "reason": r["excluded"]} for r in drop]}, open(sys.argv[3], "w"), indent=1)
print(f"keep {len(keep)}, excluded {len(drop)}", file=sys.stderr)
