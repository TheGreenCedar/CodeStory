import 'dart:math';

int numberIn() {
  return 1;
}

void numberOut(int num) {
  print(num);
}

void stringOut(String value) {
  print(value);
}

class GameObject {
  void announce() {}
}

class Field extends GameObject {
  int left = 9;

  int sameInRow(int token, int amount) {
    return token * amount;
  }

  bool makeMove(int row, int col, int token) {
    if (token == 0) {
      return false;
    }
    left -= row + col;
    sameInRow(token, 3);
    return true;
  }
}

abstract class Player {
  bool turn(Field field, int token);
}

class HumanPlayer implements Player {
  bool turn(Field field, int token) {
    return field.makeMove(0, 0, token);
  }
}

class ArtificialPlayer implements Player {
  int minMax(Field field, int token, int depth) {
    if (depth == 0) {
      return 0;
    }
    return minMax(field, token, depth - 1);
  }

  bool turn(Field field, int token) {
    minMax(field, token, 3);
    return true;
  }
}

class TicTacToe extends GameObject {
  final field = Field();

  void run() {
    numberIn();
    stringOut('start');
    Random().nextInt(3);
  }
}

void main() {
  final game = TicTacToe();
  game.run();
}
