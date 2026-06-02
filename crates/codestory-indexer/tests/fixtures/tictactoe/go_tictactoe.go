package main

import (
	"fmt"
	"math/rand"
)

func numberIn() int {
	return 1
}

func numberOut(num int) {
	fmt.Print(num)
}

func stringOut(value string) {
	fmt.Print(value)
}

type GameObject struct{}

func (GameObject) announce() {}

type Field struct {
	grid  [][]int
	left  int
}

func (f *Field) opponent(token int) int {
	if token == 1 {
		return 4
	}
	if token == 4 {
		return 1
	}
	return 0
}

func (f *Field) sameInRow(token int, amount int) int {
	total := token * amount
	count := 0
	for i := 0; i < 3; i++ {
		if f.grid[i][0]+f.grid[i][1]+f.grid[i][2] == total {
			count++
		}
	}
	return count
}

func (f *Field) makeMove(row int, col int, token int) bool {
	if token == 0 {
		return false
	}
	f.grid[row][col] = token
	f.left--
	f.sameInRow(token, 3)
	return true
}

type Player interface {
	turn(field *Field, token int) bool
}

type HumanPlayer struct{}

func (HumanPlayer) turn(field *Field, token int) bool {
	return field.makeMove(0, 0, token)
}

type ArtificialPlayer struct{}

func (p ArtificialPlayer) minMax(field *Field, token int, depth int) int {
	if depth == 0 {
		return 0
	}
	return p.minMax(field, token, depth-1)
}

func (p ArtificialPlayer) turn(field *Field, token int) bool {
	p.minMax(field, token, 3)
	return true
}

type TicTacToe struct {
	field *Field
}

func (t *TicTacToe) run() {
	t.field = &Field{grid: make([][]int, 3), left: 9}
	numberIn()
	stringOut("start")
	rand.Intn(3)
}

func main() {
	game := &TicTacToe{}
	game.run()
}
