import { randomInt } from "./random";
import { helper } from "./helper";

type Token = 0 | 1 | 4;

type Move = {
  row: number;
  col: number;
};

interface CellProps {
  value: Token;
  move: Move;
  onPick: (move: Move) => void;
}

function tokenLabel(value: Token): string {
  if (value === 1) {
    return "X";
  }
  if (value === 4) {
    return "O";
  }
  return " ";
}

function Cell(props: CellProps) {
  return (
    <button className="cell" onClick={() => props.onPick(props.move)}>
      {tokenLabel(props.value)}
    </button>
  );
}

function Row(props: { cells: Token[]; row: number; onPick: (move: Move) => void }) {
  return (
    <div className="row">
      <Cell value={props.cells[0]} move={{ row: props.row, col: 0 }} onPick={props.onPick} />
      <Cell value={props.cells[1]} move={{ row: props.row, col: 1 }} onPick={props.onPick} />
      <Cell value={props.cells[2]} move={{ row: props.row, col: 2 }} onPick={props.onPick} />
    </div>
  );
}

class GameObject {
  announce(): void {
    helper();
  }
}

class Field extends GameObject {
  private grid: Token[][];
  private left: number;

  constructor() {
    super();
    this.grid = [
      [0, 0, 0],
      [0, 0, 0],
      [0, 0, 0],
    ];
    this.left = 9;
  }

  inRange(move: Move): boolean {
    return move.row >= 0 && move.row < 3 && move.col >= 0 && move.col < 3;
  }

  isEmpty(move: Move): boolean {
    return this.grid[move.row][move.col] === 0;
  }

  isDraw(): boolean {
    return this.left === 0;
  }

  rows(): Token[][] {
    return this.grid;
  }

  makeMove(move: Move, token: Token): boolean {
    if (!this.inRange(move) || !this.isEmpty(move)) {
      return false;
    }
    this.grid[move.row][move.col] = token;
    this.left -= 1;
    this.isDraw();
    return true;
  }

  clearMove(move: Move): void {
    if (!this.inRange(move) || this.isEmpty(move)) {
      return;
    }
    this.grid[move.row][move.col] = 0;
    this.left += 1;
  }
}

class ArtificialPlayer extends GameObject {
  evaluate(field: Field, move: Move): number {
    if (!field.isEmpty(move)) {
      return -1;
    }
    return randomInt(0, 2);
  }

  turn(field: Field): Move {
    let best: Move = { row: 0, col: 0 };
    let bestValue = -1;
    for (let row = 0; row < 3; row += 1) {
      for (let col = 0; col < 3; col += 1) {
        const move = { row, col };
        const value = this.evaluate(field, move);
        if (value > bestValue) {
          bestValue = value;
          best = move;
        }
      }
    }
    return best;
  }
}

export class TicTacToe extends GameObject {
  private field: Field;
  private opponent: ArtificialPlayer;

  constructor() {
    super();
    this.field = new Field();
    this.opponent = new ArtificialPlayer();
  }

  pick(move: Move): void {
    if (!this.field.makeMove(move, 1)) {
      return;
    }
    const reply = this.opponent.turn(this.field);
    this.field.makeMove(reply, 4);
    this.field.clearMove(reply);
    this.announce();
  }

  board(): Token[][] {
    return this.field.rows();
  }
}

export function Board(props: { game: TicTacToe }) {
  const rows = props.game.board();
  return (
    <main className="board">
      <Row cells={rows[0]} row={0} onPick={(move) => props.game.pick(move)} />
      <Row cells={rows[1]} row={1} onPick={(move) => props.game.pick(move)} />
      <Row cells={rows[2]} row={2} onPick={(move) => props.game.pick(move)} />
    </main>
  );
}

export function main(): void {
  const game = new TicTacToe();
  game.pick({ row: 1, col: 1 });
}
