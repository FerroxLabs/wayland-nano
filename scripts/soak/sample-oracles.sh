#!/bin/sh
set -eu
pid=$1
nano_home=$2
rss_kb=$(ps -o rss= -p "$pid" | tr -d ' ')
threads=$(ps -o nlwp= -p "$pid" | tr -d ' ')
fds=$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')
home_bytes=$(du -sb "$nano_home" 2>/dev/null | awk '{print $1}')
children=$(ps -o pid= --ppid "$pid" | tr '\n' ',' | sed 's/,$//')
printf '{"at":"%s","pid":%s,"privateWorkingSetBytes":%s,"workingSetBytes":%s,"handles":null,"threads":%s,"openFds":%s,"nanoHomeBytes":%s,"descendants":"%s"}\n' "$(date -u +%FT%TZ)" "$pid" "$((rss_kb*1024))" "$((rss_kb*1024))" "$threads" "$fds" "${home_bytes:-0}" "$children"
