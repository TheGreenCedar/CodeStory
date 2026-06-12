source ./random.sh

numberIn() {
  echo 1
}

numberOut() {
  printf "%s\n" "$1"
}

stringOut() {
  printf "%s\n" "$1"
}

sameInRow() {
  token="$1"
  amount="$2"
  echo "$((token * amount))"
}

makeMove() {
  row="$1"
  col="$2"
  token="$3"
  if [ "$token" -eq 0 ]; then
    return 1
  fi
  sameInRow "$token" 3
  echo "$row:$col"
}

turn() {
  makeMove 0 0 "$1"
}

minMax() {
  depth="$3"
  if [ "$depth" -eq 0 ]; then
    echo 0
    return
  fi
  minMax "$1" "$2" "$((depth - 1))"
}

run() {
  numberIn
  stringOut "start"
  minMax field 1 3
}

main() {
  run
}

main "$@"
