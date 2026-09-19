#!/usr/bin/env bash
# Reads a cargo/clippy JSON message stream on stdin, prints one lint code per line.
#
# The whole stream is validated strictly as JSON: every record must be an
# object carrying a known cargo reason, every compiler-message must have a
# valid diagnostic shape and level, no compiler error may appear, and the
# stream must end with exactly one successful build-finished record. Anything
# else is an ERROR (exit 2), never silence.
set -euo pipefail

input="$(cat)"
if ! codes="$(printf '%s' "$input" | jq -r -s -e '
      def levels: ["error","warning","note","help","failure-note"];
      def reasons: ["compiler-artifact","compiler-message","build-script-executed","build-finished"];
      if length == 0 then error("no records on stdin") else . end
      | if any(.[]; type != "object") then error("record is not an object") else . end
      | if any(.[]; (.reason | type) != "string" or (.reason as $r | reasons | index($r) | not))
        then error("unknown or missing cargo reason") else . end
      | [.[] | select(.reason == "compiler-message")] as $msgs
      | if any($msgs[]; (.message | type) != "object")
        then error("compiler-message.message is not an object") else . end
      | if any($msgs[]; (.message.level | type) != "string" or (.message.level as $l | levels | index($l) | not))
        then error("invalid diagnostic level") else . end
      | if any($msgs[]; .message.level == "error" or .message.level == "failure-note")
        then error("compiler error present in diagnostic stream") else . end
      | if ([.[] | select(.reason == "build-finished")] | length) != 1
        then error("expected exactly one build-finished record") else . end
      | if .[-1].reason != "build-finished"
        then error("stream does not end with build-finished") else . end
      | if .[-1].success != true
        then error("build-finished reports failure") else . end
      | $msgs[] | .message.code.code // empty
    ' 2>&1)"; then
  printf 'FAIL codes: %s\n' "$codes" >&2
  exit 2
fi

if [ -n "$codes" ]; then
  printf '%s\n' "$codes"
fi
exit 0
