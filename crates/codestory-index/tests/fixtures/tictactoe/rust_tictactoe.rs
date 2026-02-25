use std::io::{self, Write};
use std::fmt;

struct GameObject;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    TOKEN_NONE = 0,
    TOKEN_PLAYER_A = 1,
    TOKEN_PLAYER_B = 4,
}

#[derive(Clone, Copy)]
struct Move {
    row: usize,
    col: usize,
}

impl Move {
    fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

fn number_in() -> i32 {
    1
}

fn number_out(num: i32) {
    let _ = write!(io::stdout(), "{}", num);
}

fn string_out(value: &str) {
    let _ = write!(io::stdout(), "{}", value);
}

#[derive(Clone)]
struct Field {
    grid: [[Token; 3]; 3],
    left: i32,
}

impl Field {
    fn new() -> Self {
        let mut field = Field {
            grid: [[Token::TOKEN_NONE; 3]; 3],
            left: 9,
        };
        for row in 0..3 {
            for col in 0..3 {
                field.grid[row][col] = Token::TOKEN_NONE;
            }
        }
        field
    }

    fn opponent(&self, token: Token) -> Token {
        if token == Token::TOKEN_PLAYER_A {
            Token::TOKEN_PLAYER_B
        } else if token == Token::TOKEN_PLAYER_B {
            Token::TOKEN_PLAYER_A
        } else {
            Token::TOKEN_NONE
        }
    }

    fn clone_field(&self) -> Self {
        let mut clone = Self::new();
        for row in 0..3 {
            for col in 0..3 {
                clone.grid[row][col] = self.grid[row][col];
            }
        }
        clone.left = self.left;
        clone
    }

    fn clear(&mut self) {
        for row in 0..3 {
            for col in 0..3 {
                self.grid[row][col] = Token::TOKEN_NONE;
            }
        }
        self.left = 9;
    }

    fn show(&self) {
        string_out("   1   2   3\n");
        for row in 0..3 {
            number_out((row + 1) as i32);
            string_out(" ");
            for col in 0..3 {
                if self.grid[row][col] == Token::TOKEN_PLAYER_A {
                    string_out(" X ");
                } else if self.grid[row][col] == Token::TOKEN_PLAYER_B {
                    string_out(" O ");
                } else {
                    string_out("   ");
                }
                if col < 2 {
                    string_out("|");
                }
            }
            if row < 2 {
                string_out("\n  -----------\n");
            }
        }
        string_out("\n\n");
    }

    fn same_in_row(&self, token: Token, amount: i32) -> i32 {
        let total = amount * token as i32;
        let mut count = 0;
        for row in 0..3 {
            if (self.grid[row][0] as i32 + self.grid[row][1] as i32 + self.grid[row][2] as i32)
                == total
            {
                count += 1;
            }
            if (self.grid[0][row] as i32 + self.grid[1][row] as i32 + self.grid[2][row] as i32)
                == total
            {
                count += 1;
            }
        }
        if (self.grid[0][0] as i32 + self.grid[1][1] as i32 + self.grid[2][2] as i32) == total {
            count += 1;
        }
        if (self.grid[2][0] as i32 + self.grid[1][1] as i32 + self.grid[0][2] as i32) == total {
            count += 1;
        }
        count
    }

    fn in_range(&self, mv: Move) -> bool {
        mv.row < 3 && mv.col < 3
    }

    fn is_empty(&self, mv: Move) -> bool {
        self.grid[mv.row][mv.col] == Token::TOKEN_NONE
    }

    fn is_full(&self) -> bool {
        self.left == 0
    }

    fn make_move(&mut self, mv: Move, token: Token) {
        if !self.in_range(mv) {
            return;
        }
        if !self.is_empty(mv) {
            return;
        }
        if token == Token::TOKEN_NONE {
            return;
        }
        if self.is_full() {
            return;
        }
        self.grid[mv.row][mv.col] = token;
        self.left -= 1;
        self.same_in_row(token, 3);
    }

    fn clear_move(&mut self, mv: Move) {
        if !self.in_range(mv) {
            return;
        }
        if self.is_empty(mv) {
            return;
        }
        if self.left == 9 {
            return;
        }
        self.grid[mv.row][mv.col] = Token::TOKEN_NONE;
        self.left += 1;
    }
}

fn check_winner(field: &Field, token: Token) -> bool {
    field.same_in_row(token, 3) > 0
}

fn is_draw(field: &Field) -> bool {
    field.is_full()
}

fn probe_check_winner(field: &Field) {
    check_winner(field, Token::TOKEN_PLAYER_A);
}

fn probe_is_draw(field: &Field) {
    is_draw(field);
}

trait Player {
    fn turn(&self, field: &Field) -> Move;
    fn token(&self) -> Token;
    fn name(&self) -> &str;
}

#[derive(Clone)]
struct HumanPlayer {
    token: Token,
    name: String,
}

impl HumanPlayer {
    fn new(token: Token, name: &str) -> Self {
        Self {
            token,
            name: name.to_string(),
        }
    }

    fn input(&self) -> Move {
        Move::new(number_in() as usize - 1, number_in() as usize - 1)
    }

    fn check(&self, field: &Field, mv: Move) -> bool {
        if !field.in_range(mv) {
            string_out("Wrong input!\n");
            return false;
        }
        if !field.is_empty(mv) {
            string_out("Is occupied!\n");
            return false;
        }
        true
    }
}

impl Player for HumanPlayer {
    fn token(&self) -> Token {
        self.token
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn turn(&self, field: &Field) -> Move {
        string_out(self.name());
        string_out("\n");
        loop {
            let mv = self.input();
            self.check(field, mv);
            if self.check(field, mv) {
                return mv;
            }
        }
    }
}

#[derive(Clone)]
struct Node {
    mv: Move,
    value: i32,
}

#[derive(Clone)]
struct ArtificialPlayer {
    token: Token,
    name: String,
}

impl ArtificialPlayer {
    fn new(token: Token, name: &str) -> Self {
        Self {
            token,
            name: name.to_string(),
        }
    }

    fn evaluate(&self, field: &Field, token: Token) -> i32 {
        if field.same_in_row(token, 3) > 0 {
            2
        } else if field.same_in_row(field.opponent(token), 2) > 0 {
            -1
        } else if field.same_in_row(token, 2) > 1 {
            1
        } else {
            0
        }
    }

    fn min_max(&self, field: &mut Field, token: Token) -> Node {
        let mut node = Node {
            mv: Move::new(0, 0),
            value: -10_000,
        };

        for row in 0..3 {
            for col in 0..3 {
                let mv = Move::new(row, col);
                if !field.is_empty(mv) {
                    continue;
                }

                field.make_move(mv, token);
                let mut turn_value = self.evaluate(field, token);
                if turn_value == 0 && !field.is_full() {
                    let child = self.min_max(field, field.opponent(token));
                    turn_value = -child.value;
                }
                field.clear_move(mv);

                if turn_value > node.value {
                    node.mv = mv;
                    node.value = turn_value;
                }
            }
        }

        node
    }
}

impl Player for ArtificialPlayer {
    fn token(&self) -> Token {
        self.token
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn turn(&self, field: &Field) -> Move {
        let mut temp_field = field.clone_field();
        let node = self.min_max(&mut temp_field, self.token);
        node.mv
    }
}

struct TicTacToe {
    field: Field,
    players: [Option<Box<dyn Player>>; 2],
}

impl TicTacToe {
    fn new() -> Self {
        Self {
            field: Field::new(),
            players: [None, None],
        }
    }

    fn start(&mut self) -> bool {
        self._reset();
        string_out("Tic Tac Toe\n\n[1] Human\n[2] Computer\n[3] Quit\n\n");

        self.players[0] = Some(Box::new(Self::_select_player(Token::TOKEN_PLAYER_A, "Player A")));
        self.players[1] = Some(Box::new(Self::_select_player(Token::TOKEN_PLAYER_B, "Player B")));
        self.players[0].is_some() && self.players[1].is_some()
    }

    fn run(&mut self) {
        self.field.show();
        let mut player_index = 0;
        for _ in 0..9 {
            let player = self.players[player_index]
                .as_ref()
                .expect("players must be initialized");
            let selected = player.turn(&self.field);
            self.field.make_move(selected, player.token());
            check_winner(&self.field, player.token());
            is_draw(&self.field);
            if check_winner(&self.field, player.token()) {
                string_out(player.name());
                string_out(" won!\n\n");
                return;
            }
            if is_draw(&self.field) {
                string_out("Game ends in draw!\n\n");
                return;
            }
            player_index = (player_index + 1) % 2;
        }
    }

    fn _reset(&mut self) {
        self.field.clear();
        self.players = [None, None];
    }

    fn _select_player(token: Token, name: &str) -> Box<dyn Player> {
        string_out("Choose ");
        string_out(name);
        string_out(": ");
        let selection = number_in();
        if selection == 1 {
            Box::new(HumanPlayer::new(token, name))
        } else {
            Box::new(ArtificialPlayer::new(token, name))
        }
    }
}

fn main() {
    let mut tictactoe = TicTacToe::new();
    while tictactoe.start() {
        tictactoe.run();
    }
}
