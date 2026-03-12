import { randomInt } from "./random";
import { helper } from "./helper";

type Token = 0 | 1 | 4;

type Move = {
  row: number;
  col: number;
};

function numberIn(): number {
  return 1;
}

function numberOut(num: number): void {
  process.stdout.write(String(num));
}

function stringOut(value: string): void {
  process.stdout.write(value);
}

class GameObject {
  announce(): void {
    helper();
  }
}

class Field extends GameObject {
  static Token = {
    NONE: 0 as Token,
    PLAYER_A: 1 as Token,
    PLAYER_B: 4 as Token,
  };

  private grid: Token[][];
  private left: number;

  constructor() {
    super();
    this.grid = Array.from({ length: 3 }, () => Array<Token>(3).fill(Field.Token.NONE));
    this.left = 9;
  }

  cloneField(): Field {
    const field = new Field();
    for (let row = 0; row < 3; row += 1) {
      for (let col = 0; col < 3; col += 1) {
        field.grid[row][col] = this.grid[row][col];
      }
    }
    field.left = this.left;
    return field;
  }

  opponent(token: Token): Token {
    if (token === Field.Token.PLAYER_A) {
      return Field.Token.PLAYER_B;
    }
    if (token === Field.Token.PLAYER_B) {
      return Field.Token.PLAYER_A;
    }
    return Field.Token.NONE;
  }

  inRange(move: Move): boolean {
    return move.row >= 0 && move.row < 3 && move.col >= 0 && move.col < 3;
  }

  isEmpty(move: Move): boolean {
    return this.grid[move.row][move.col] === Field.Token.NONE;
  }

  isDraw(): boolean {
    return this.left === 0;
  }

  sameInRow(token: Token, amount: number): number {
    const total = token * amount;
    let count = 0;
    for (let i = 0; i < 3; i += 1) {
      if (this.grid[i][0] + this.grid[i][1] + this.grid[i][2] === total) {
        count += 1;
      }
      if (this.grid[0][i] + this.grid[1][i] + this.grid[2][i] === total) {
        count += 1;
      }
    }
    if (this.grid[0][0] + this.grid[1][1] + this.grid[2][2] === total) {
      count += 1;
    }
    if (this.grid[2][0] + this.grid[1][1] + this.grid[0][2] === total) {
      count += 1;
    }
    return count;
  }

  makeMove(move: Move, token: Token): boolean {
    if (!this.inRange(move) || !this.isEmpty(move) || token === Field.Token.NONE) {
      return false;
    }
    this.grid[move.row][move.col] = token;
    this.left -= 1;
    this.sameInRow(token, 3);
    this.isDraw();
    return true;
  }

  clearMove(move: Move): void {
    if (!this.inRange(move) || this.isEmpty(move)) {
      return;
    }
    this.grid[move.row][move.col] = Field.Token.NONE;
    this.left += 1;
  }

  show(): void {
    stringOut("   1   2   3\n");
    for (let row = 0; row < 3; row += 1) {
      numberOut(row + 1);
      stringOut(" ");
      for (let col = 0; col < 3; col += 1) {
        const value = this.grid[row][col];
        if (value === Field.Token.PLAYER_A) {
          stringOut(" X ");
        } else if (value === Field.Token.PLAYER_B) {
          stringOut(" O ");
        } else {
          stringOut("   ");
        }
        if (col < 2) {
          stringOut("|");
        }
      }
      stringOut("\n");
    }
    stringOut("\n");
  }
}

abstract class Player extends GameObject {
  token: Token;
  name: string;

  constructor(token: Token, name: string) {
    super();
    this.token = token;
    this.name = name;
  }

  abstract turn(field: Field): Move;
}

class HumanPlayer extends Player {
  input(): Move {
    return { row: numberIn() - 1, col: numberIn() - 1 };
  }

  check(field: Field, move: Move): boolean {
    if (!field.inRange(move)) {
      stringOut("Wrong input!\n");
      return false;
    }
    if (!field.isEmpty(move)) {
      stringOut("Is occupied!\n");
      return false;
    }
    return true;
  }

  turn(field: Field): Move {
    stringOut(this.name);
    stringOut("\n");
    while (true) {
      const move = this.input();
      this.check(field, move);
      if (this.check(field, move)) {
        return move;
      }
    }
  }
}

class ArtificialPlayer extends Player {
  evaluate(field: Field, token: Token): number {
    if (field.sameInRow(token, 3)) {
      return 2;
    }
    if (field.sameInRow(field.opponent(token), 2)) {
      return -1;
    }
    if (field.sameInRow(token, 2) > 1) {
      return 1;
    }
    return 0;
  }

  minMax(field: Field, token: Token): { move: Move; value: number } {
    let best = { move: { row: 0, col: 0 }, value: -10000 };
    let sameMove = 0;

    for (let row = 0; row < 3; row += 1) {
      for (let col = 0; col < 3; col += 1) {
        const move = { row, col };
        if (!field.isEmpty(move)) {
          continue;
        }

        field.makeMove(move, token);
        let turnValue = this.evaluate(field, token);
        if (turnValue === 0 && !field.isDraw()) {
          turnValue = -this.minMax(field, field.opponent(token)).value;
        }
        field.clearMove(move);

        if (turnValue > best.value) {
          best = { move, value: turnValue };
          sameMove = 1;
        } else if (turnValue === best.value) {
          sameMove += 1;
          if (randomInt(0, sameMove - 1) === 0) {
            best = { move, value: turnValue };
          }
        }
      }
    }

    return best;
  }

  turn(field: Field): Move {
    const temp = field.cloneField();
    const node = this.minMax(temp, this.token);
    return node.move;
  }
}

export class TicTacToe extends GameObject {
  private field: Field;
  private players: [Player | null, Player | null];

  constructor() {
    super();
    this.field = new Field();
    this.players = [null, null];
  }

  checkWinner(player: Player): boolean {
    return this.field.sameInRow(player.token, 3) > 0;
  }

  isDraw(): boolean {
    return this.field.isDraw();
  }

  selectPlayer(token: Token, name: string): Player | null {
    const selection = numberIn();
    if (selection === 1) {
      return new HumanPlayer(token, name);
    }
    if (selection === 2) {
      return new ArtificialPlayer(token, name);
    }
    return null;
  }

  start(): boolean {
    this.players[0] = this.selectPlayer(Field.Token.PLAYER_A, "Player A");
    this.players[1] = this.selectPlayer(Field.Token.PLAYER_B, "Player B");
    return Boolean(this.players[0] && this.players[1]);
  }

  probeCalls(): void {
    this.checkWinner(this.players[0] as Player);
  }

  run(): void {
    this.field.show();
    let playerIndex = 0;
    for (let i = 0; i < 9; i += 1) {
      const player = this.players[playerIndex];
      if (!player) {
        return;
      }
      const move = player.turn(this.field);
      this.field.makeMove(move, player.token);
      this.checkWinner(player);
      this.isDraw();
      if (this.checkWinner(player)) {
        this.announce();
        stringOut(player.name + " won!\n");
        return;
      }
      if (this.isDraw()) {
        this.announce();
        stringOut("Game ends in draw!\n");
        return;
      }
      playerIndex = (playerIndex + 1) % 2;
    }
  }
}

export function main(): void {
  const game = new TicTacToe();
  if (game.start()) {
    game.run();
  }
}
