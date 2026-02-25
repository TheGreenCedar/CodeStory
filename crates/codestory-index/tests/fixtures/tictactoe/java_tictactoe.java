import java.util.concurrent.ThreadLocalRandom;


public class Entry {
    public static int numberIn() {
        return 1;
    }

    public static void numberOut(int num) {
        System.out.print(num);
    }

    public static void stringOut(String value) {
        System.out.print(value);
    }

    static class GameObject {
        void announce() {}
    }

    static class Field extends GameObject {
        enum Token {
            TOKEN_NONE(0),
            TOKEN_PLAYER_A(1),
            TOKEN_PLAYER_B(4);

            private final int value;

            Token(int value) {
                this.value = value;
            }
        }

        static class Move {
            int row;
            int col;
        }

        Token[][] _grid = new Token[3][3];
        int _left = 9;

        Field() {
            for (int row = 0; row < 3; row++) {
                for (int col = 0; col < 3; col++) {
                    _grid[row][col] = Token.TOKEN_NONE;
                }
            }
        }

        Token opponent(Token token) {
            if (token == Token.TOKEN_PLAYER_A) {
                return Token.TOKEN_PLAYER_B;
            }
            if (token == Token.TOKEN_PLAYER_B) {
                return Token.TOKEN_PLAYER_A;
            }
            return Token.TOKEN_NONE;
        }

        Field cloneField() {
            Field field = new Field();
            for (int row = 0; row < 3; row++) {
                for (int col = 0; col < 3; col++) {
                    field._grid[row][col] = _grid[row][col];
                }
            }
            field._left = _left;
            return field;
        }

        void clear() {
            for (int row = 0; row < 3; row++) {
                for (int col = 0; col < 3; col++) {
                    _grid[row][col] = Token.TOKEN_NONE;
                }
            }
            _left = 9;
        }

        boolean inRange(Move move) {
            return move.row >= 0 && move.row < 3 && move.col >= 0 && move.col < 3;
        }

        boolean isEmpty(Move move) {
            return _grid[move.row][move.col] == Token.TOKEN_NONE;
        }

        boolean isDraw() {
            return _left == 0;
        }

        int sameInRow(Token token, int amount) {
            int total = amount * token.value;
            int count = 0;

            for (int i = 0; i < 3; i++) {
                if (_grid[i][0].value + _grid[i][1].value + _grid[i][2].value == total) {
                    count += 1;
                }
                if (_grid[0][i].value + _grid[1][i].value + _grid[2][i].value == total) {
                    count += 1;
                }
            }

            if (_grid[0][0].value + _grid[1][1].value + _grid[2][2].value == total) {
                count += 1;
            }
            if (_grid[2][0].value + _grid[1][1].value + _grid[0][2].value == total) {
                count += 1;
            }
            return count;
        }

        void makeMove(Move move, Token token) {
            if (!inRange(move) || !isEmpty(move) || token == Token.TOKEN_NONE || isDraw()) {
                return;
            }
            _grid[move.row][move.col] = token;
            _left -= 1;
            sameInRow(token, 3);
            isDraw();
        }

        void clearMove(Move move) {
            if (!inRange(move) || isEmpty(move)) {
                return;
            }
            _grid[move.row][move.col] = Token.TOKEN_NONE;
            _left += 1;
        }
    }

    static abstract class Player extends GameObject {
        Field.Token token;
        String name;

        Player(Field.Token token, String name) {
            this.token = token;
            this.name = name;
        }

        abstract Field.Move turn(Field field);
    }

    static class HumanPlayer extends Player {
        HumanPlayer(Field.Token token, String name) {
            super(token, name);
        }

        Field.Move turn(Field field) {
            while (true) {
                Field.Move move = _input();
                _check(field, move);
                if (_check(field, move)) {
                    return move;
                }
            }
        }

        Field.Move _input() {
            Field.Move move = new Field.Move();
            move.row = numberIn() - 1;
            move.col = numberIn() - 1;
            return move;
        }

        boolean _check(Field field, Field.Move move) {
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
    }

    static class ArtificialPlayer extends Player {
        static class Node {
            Field.Move move;
            int value;
        }

        ArtificialPlayer(Field.Token token, String name) {
            super(token, name);
        }

        Field.Move turn(Field field) {
            Field tempField = field.cloneField();
            Node node = _minMax(tempField, token);
            return node.move;
        }

        Node _minMax(Field field, Field.Token token) {
            Node node = new Node();
            node.value = -10000;
            node.move = new Field.Move();

            for (int row = 0; row < 3; row++) {
                for (int col = 0; col < 3; col++) {
                    Field.Move move = new Field.Move();
                    move.row = row;
                    move.col = col;

                    if (!field.isEmpty(move)) {
                        continue;
                    }

                    field.makeMove(move, token);
                    int turnValue = evaluate(field, token);
                    if (turnValue == 0 && !field.isDraw()) {
                        turnValue = -_minMax(field, field.opponent(token)).value;
                    }
                    field.clearMove(move);

                    if (turnValue > node.value) {
                        node.move = move;
                        node.value = turnValue;
                    } else if (turnValue == node.value && ThreadLocalRandom.current().nextBoolean()) {
                        node.move = move;
                    }
                }
            }
            return node;
        }

        int evaluate(Field field, Field.Token token) {
            if (field.sameInRow(token, 3) > 0) {
                return 2;
            }
            if (field.sameInRow(field.opponent(token), 2) > 0) {
                return -1;
            }
            if (field.sameInRow(token, 2) > 1) {
                return 1;
            }
            return 0;
        }
    }

    static class TicTacToe extends GameObject {
        Field field = new Field();
        Player[] players = new Player[2];

        boolean start() {
            field.clear();
            players[0] = new HumanPlayer(Field.Token.TOKEN_PLAYER_A, "Player A");
            players[1] = new ArtificialPlayer(Field.Token.TOKEN_PLAYER_B, "Player B");
            return players[0] != null && players[1] != null;
        }

        boolean checkWinner(Player player) {
            return field.sameInRow(player.token, 3) > 0;
        }

        boolean isDraw() {
            return field.isDraw();
        }

        void probeCalls() {
            checkWinner(players[0]);
        }

        void run() {
            int playerIndex = 0;
            for (int i = 0; i < 9; i++) {
                Player player = players[playerIndex];
                Field.Move move = player.turn(field);
                field.makeMove(move, player.token);
                checkWinner(player);
                isDraw();
                if (checkWinner(player)) {
                    announce();
                    stringOut(player.name + " won!\n");
                    return;
                }
                if (isDraw()) {
                    announce();
                    stringOut("Game ends in draw!\n");
                    return;
                }
                playerIndex = (playerIndex + 1) % 2;
            }
        }
    }

    public static void main(String[] args) {
        TicTacToe game = new TicTacToe();
        if (game.start()) {
            game.run();
        }
    }
}
