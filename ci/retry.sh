#!/usr/bin/env bash
# ci/retry.sh COMMAND...: run COMMAND up to four times, waiting 20, 40 and 60 s
# between attempts; exit with the last attempt's status. For network fetches in
# CI: one transient error (a download answering 403) fails a job, and a job
# without rows makes the publication gate refuse a whole sweep.
# ($RETRY_WAIT_S is the first wait, for tests.)
set -u
wait=${RETRY_WAIT_S:-20}
for attempt in 1 2 3; do
  "$@" && exit 0
  echo "::warning::attempt $attempt failed, retrying: $1" >&2
  sleep $((attempt * wait))
done
exec "$@"
