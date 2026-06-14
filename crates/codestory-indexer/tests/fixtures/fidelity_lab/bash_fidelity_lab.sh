source ./logger.sh

notify() {
  event="$1"
  printf "%s\n" "$event"
}

save() {
  event="$1"
  printf "%s\n" "$event"
}

decorate() {
  event="$1"
  printf "%s\n" "$event"
}

run() {
  event="$1"
  notify "$event"
  save "$event"
  decorate "$event"
}

orchestrate_bash() {
  run "ready"
}

orchestrate_bash "$@"
