#!/bin/sh
# A process extension in plain POSIX sh: the reference `pi-example-questions` plugin
# (coding-agent/examples/plugins/pi-example-questions/src/session.ts) over Pipkin's JSON-lines
# extension protocol (see docs/native-extensions.md).
#
# It asks a choice, a yes/no and a line of text, reports each answer as a notice, sets a status
# line, and then waits to be deactivated. Answers are read with plain string handling, so this
# fixture only understands answers without quotes or backslashes.
set -u

# hello -> hello, then wait for activate.
IFS= read -r line || exit 1
case $line in *'"type":"hello"'*) ;; *) exit 2 ;; esac
printf '%s\n' '{"type":"hello","api":1}'
IFS= read -r line || exit 1
case $line in *'"type":"activate"'*) ;; *) exit 3 ;; esac

next_id=0
reply=

# call '<"method":...,"params":...>': send a request and wait for its reply in $reply.
call() {
	next_id=$((next_id + 1))
	printf '{"id":%d,%s}\n' "$next_id" "$1"
	while :; do
		IFS= read -r reply || exit 0
		case $reply in
		*'"type":"deactivate"'*) exit 0 ;;
		"{\"id\":$next_id,"*) return 0 ;;
		esac
	done
}

# The value of "result" in $reply, without quotes: mint, true, null, ...
result() {
	r=${reply#*\"result\":}
	r=${r%\}}
	case $r in \"*\") r=${r#\"}; r=${r%\"} ;; esac
	printf '%s' "$r"
}

notify() {
	call "\"method\":\"notify\",\"params\":{\"level\":\"info\",\"message\":\"$1\"}"
}

call '"method":"ask","params":{"kind":"select","title":"Pick a flavor","message":"The example extension is asking.","options":[{"value":"mint","label":"Mint","description":"cool and sharp"},{"value":"vanilla","label":"Vanilla","description":"plain and sweet"},{"value":"pepper","label":"Pepper"}]}'
flavor=$(result)
if [ "$flavor" = null ]; then notify 'No flavor was picked.'; else notify "You picked $flavor."; fi

call '"method":"ask","params":{"kind":"confirm","title":"Keep going?","message":"Answer yes or no."}'
keep=$(result)
if [ "$keep" = null ]; then notify 'No answer to keep going.'; else notify "Keep going: $keep."; fi

call '"method":"ask","params":{"kind":"input","title":"What should it be called?","placeholder":"a name","defaultValue":"pipsqueak"}'
name=$(result)
if [ "$name" = null ]; then notify 'No name was given.'; else notify "Called $name."; fi

[ "$flavor" = null ] && flavor=none
call "\"method\":\"setStatus\",\"params\":{\"key\":\"example-questions\",\"text\":\"flavor $flavor\"}"

# Like a facet, stay until the session deactivates it.
while IFS= read -r line; do
	case $line in *'"type":"deactivate"'*) exit 0 ;; esac
done
exit 0
