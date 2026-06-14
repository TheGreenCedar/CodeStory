import Foundation

func numberIn() -> Int {
    return 1
}

func numberOut(_ num: Int) {
    print(num)
}

func stringOut(_ value: String) {
    print(value)
}

class GameObject {
    func announce() {}
}

class Field: GameObject {
    var left = 9

    func sameInRow(token: Int, amount: Int) -> Int {
        return token * amount
    }

    func makeMove(row: Int, col: Int, token: Int) -> Bool {
        if token == 0 {
            return false
        }
        left -= row + col
        sameInRow(token: token, amount: 3)
        return true
    }
}

protocol Player {
    func turn(field: Field, token: Int) -> Bool
}

class HumanPlayer: Player {
    func turn(field: Field, token: Int) -> Bool {
        return field.makeMove(row: 0, col: 0, token: token)
    }
}

class ArtificialPlayer: Player {
    func minMax(field: Field, token: Int, depth: Int) -> Int {
        if depth == 0 {
            return 0
        }
        return minMax(field: field, token: token, depth: depth - 1)
    }

    func turn(field: Field, token: Int) -> Bool {
        minMax(field: field, token: token, depth: 3)
        return true
    }
}

class TicTacToe: GameObject {
    let field = Field()

    func run() {
        numberIn()
        stringOut("start")
        Int.random(in: 0..<3)
    }
}

func main() {
    let game = TicTacToe()
    game.run()
}
