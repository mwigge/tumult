#!/bin/sh
# A config store made of files, for the DR runs of the docs pictures. The
# store answers while the file `up` is there. `store` holds the markers the
# store kept, `ledger` the markers it confirmed, and `backup` a copy of the
# store. data.py makes a fresh lab before each run.
#
# Usage: lab.sh <lab dir> <command>
#   up                print up while the store answers
#   lost              the newest ledger time minus the newest marker kept,
#                     in seconds; fails while the store does not answer
#   write <seconds>   write a marker every half second, for that long
#   backup            copy the store to the backup
#   restore           make the store again from the backup
#   replica           stop one replica of three: the store keeps answering
set -eu
lab=$1
command=$2

newest() { tail -n 1 "$1"; }

case $command in
up)
  test -f "$lab/up"
  echo up
  ;;
lost)
  test -f "$lab/up"
  test -s "$lab/store"
  awk -v ledger="$(newest "$lab/ledger")" -v kept="$(newest "$lab/store")" \
    'BEGIN { lost = ledger - kept; printf "%.2f\n", lost < 0 ? 0 : lost }'
  ;;
write)
  end=$(($(date +%s) + $3))
  while [ "$(date +%s)" -lt "$end" ]; do
    marker=$(date +%s.%N)
    echo "$marker" >>"$lab/store"
    echo "$marker" >>"$lab/ledger"
    sleep 0.5
  done
  echo "wrote markers for $3 s"
  ;;
backup)
  cp "$lab/store" "$lab/backup"
  echo "backed up $(wc -l <"$lab/backup") markers"
  ;;
restore)
  cp "$lab/backup" "$lab/store"
  : >"$lab/up"
  echo "restored $(wc -l <"$lab/store") markers"
  ;;
replica)
  echo "stopped replica 2 of 3; the other two answer"
  ;;
*)
  echo "unknown command $command" >&2
  exit 2
  ;;
esac
