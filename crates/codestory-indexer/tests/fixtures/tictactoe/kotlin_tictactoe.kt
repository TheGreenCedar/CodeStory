package tictactoe

import kotlin.random.Random

fun numberIn(): Int = 1

fun numberOut(num: Int) {
    println(num)
}

fun stringOut(value: String) {
    println(value)
}

open class GameObject {
    fun announce() {}
}

class Field : GameObject() {
    var left: Int = 9

    fun sameInRow(token: Int, amount: Int): Int {
        return token * amount
    }

    fun makeMove(row: Int, col: Int, token: Int): Boolean {
        if (token == 0) {
            return false
        }
        left -= row + col
        sameInRow(token, 3)
        return true
    }
}

interface Player {
    fun turn(field: Field, token: Int): Boolean
}

class HumanPlayer : Player {
    override fun turn(field: Field, token: Int): Boolean {
        return field.makeMove(0, 0, token)
    }
}

class ArtificialPlayer : Player {
    fun minMax(field: Field, token: Int, depth: Int): Int {
        if (depth == 0) {
            return 0
        }
        return minMax(field, token, depth - 1)
    }

    override fun turn(field: Field, token: Int): Boolean {
        minMax(field, token, 3)
        return true
    }
}

class TicTacToe : GameObject() {
    val field = Field()

    fun run() {
        numberIn()
        stringOut("start")
        Random.nextInt(3)
    }
}

fun main() {
    val game = TicTacToe()
    game.run()
}
