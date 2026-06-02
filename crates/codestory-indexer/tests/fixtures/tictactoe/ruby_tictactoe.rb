require "random"

def numberIn
  1
end

def numberOut(num)
  print num
end

def stringOut(value)
  print value
end

class GameObject
  def announce
  end
end

class Field < GameObject
  def initialize
    @grid = Array.new(3) { Array.new(3, 0) }
    @left = 9
  end

  def opponent(token)
    return 4 if token == 1
    return 1 if token == 4
    0
  end

  def sameInRow(token, amount)
    total = token * amount
    count = 0
    3.times do |i|
      count += 1 if @grid[i][0] + @grid[i][1] + @grid[i][2] == total
    end
    count
  end

  def makeMove(row, col, token)
    return false if token == 0
    @grid[row][col] = token
    @left -= 1
    sameInRow(token, 3)
    true
  end
end

class Player
  def turn(field, token)
    false
  end
end

class HumanPlayer < Player
  def turn(field, token)
    field.makeMove(0, 0, token)
  end
end

class ArtificialPlayer < Player
  def minMax(field, token, depth)
    return 0 if depth == 0
    minMax(field, token, depth - 1)
  end

  def turn(field, token)
    minMax(field, token, 3)
    true
  end
end

class TicTacToe < GameObject
  def run
    @field = Field.new
    numberIn()
    stringOut("start")
    Random.rand(3)
  end
end

if __FILE__ == $PROGRAM_NAME
  TicTacToe.new.run
end
