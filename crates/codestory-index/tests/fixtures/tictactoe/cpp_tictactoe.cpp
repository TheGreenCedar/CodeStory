#include <array>
#include <iostream>
#include <random>
#include <string>
#include <memory>

class GameObject {
public:
    virtual ~GameObject() = default;
    virtual void announce() const {
        std::cout << "announce\n";
    }
};

class Field : public GameObject {
public:
    enum Token {
        TOKEN_NONE = 0,
        TOKEN_PLAYER_A = 1,
        TOKEN_PLAYER_B = 4,
    };

    struct Move {
        int row;
        int col;
    };

    Field() : left_(9) {
        clear();
    }

    Field clone_field() const {
        Field copy;
        copy.grid_ = grid_;
        copy.left_ = left_;
        return copy;
    }

    Token opponent(Token token) const {
        if (token == TOKEN_PLAYER_A) {
            return TOKEN_PLAYER_B;
        }
        if (token == TOKEN_PLAYER_B) {
            return TOKEN_PLAYER_A;
        }
        return TOKEN_NONE;
    }

    void clear() {
        for (auto& row : grid_) {
            row.fill(TOKEN_NONE);
        }
        left_ = 9;
    }

    bool in_range(const Move& move) const {
        return move.row >= 0 && move.row < 3 && move.col >= 0 && move.col < 3;
    }

    bool is_empty(const Move& move) const {
        return grid_[move.row][move.col] == TOKEN_NONE;
    }

    bool is_draw() const {
        return left_ == 0;
    }

    int same_in_row(Token token, int amount) const {
        const int total = amount * static_cast<int>(token);
        int count = 0;

        for (int i = 0; i < 3; ++i) {
            if (static_cast<int>(grid_[i][0]) + static_cast<int>(grid_[i][1]) + static_cast<int>(grid_[i][2]) == total) {
                ++count;
            }
            if (static_cast<int>(grid_[0][i]) + static_cast<int>(grid_[1][i]) + static_cast<int>(grid_[2][i]) == total) {
                ++count;
            }
        }

        if (static_cast<int>(grid_[0][0]) + static_cast<int>(grid_[1][1]) + static_cast<int>(grid_[2][2]) == total) {
            ++count;
        }
        if (static_cast<int>(grid_[2][0]) + static_cast<int>(grid_[1][1]) + static_cast<int>(grid_[0][2]) == total) {
            ++count;
        }
        return count;
    }

    bool make_move(const Move& move, Token token) {
        if (!in_range(move)) {
            return false;
        }
        if (!is_empty(move)) {
            return false;
        }
        if (token == TOKEN_NONE) {
            return false;
        }

        grid_[move.row][move.col] = token;
        --left_;
        same_in_row(token, 3);
        is_draw();
        return true;
    }

    void clear_move(const Move& move) {
        if (!in_range(move)) {
            return;
        }
        if (is_empty(move)) {
            return;
        }
        grid_[move.row][move.col] = TOKEN_NONE;
        ++left_;
    }

private:
    std::array<std::array<Token, 3>, 3> grid_{};
    int left_;
};

class Player : public GameObject {
public:
    Player(Field::Token token, std::string name) : token_(token), name_(std::move(name)) {}
    virtual ~Player() = default;

    virtual Field::Move turn(Field& field) = 0;

    Field::Token token() const {
        return token_;
    }

    const std::string& name() const {
        return name_;
    }

protected:
    Field::Token token_;
    std::string name_;
};

class HumanPlayer : public Player {
public:
    HumanPlayer(Field::Token token, const std::string& name) : Player(token, name) {}

    Field::Move turn(Field& field) override {
        while (true) {
            const Field::Move move = input();
            check(field, move);
            if (check(field, move)) {
                return move;
            }
        }
    }

private:
    static Field::Move input() {
        return Field::Move{0, 0};
    }

    static bool check(Field& field, const Field::Move& move) {
        if (!field.in_range(move)) {
            std::cout << "Wrong input\n";
            return false;
        }
        if (!field.is_empty(move)) {
            std::cout << "Occupied\n";
            return false;
        }
        return true;
    }
};

class ArtificialPlayer : public Player {
public:
    struct Node {
        Field::Move move;
        int value;
    };

    ArtificialPlayer(Field::Token token, const std::string& name) : Player(token, name) {}

    Field::Move turn(Field& field) override {
        Field temp = field.clone_field();
        const Node node = min_max(temp, token_);
        return node.move;
    }

private:
    Node min_max(Field& field, Field::Token token) {
        Node node{{0, 0}, -10000};
        int same_move = 0;

        for (int row = 0; row < 3; ++row) {
            for (int col = 0; col < 3; ++col) {
                Field::Move move{row, col};
                if (!field.is_empty(move)) {
                    continue;
                }

                field.make_move(move, token);
                int turn_value = evaluate(field, token);
                if (turn_value == 0 && !field.is_draw()) {
                    turn_value = -min_max(field, field.opponent(token)).value;
                }
                field.clear_move(move);

                if (turn_value > node.value) {
                    node = Node{move, turn_value};
                    same_move = 1;
                } else if (turn_value == node.value) {
                    ++same_move;
                    if (same_move == 1) {
                        node = Node{move, turn_value};
                    }
                }
            }
        }

        return node;
    }

    static int evaluate(Field& field, Field::Token token) {
        if (field.same_in_row(token, 3) > 0) {
            return 2;
        }
        if (field.same_in_row(field.opponent(token), 2) > 0) {
            return -1;
        }
        if (field.same_in_row(token, 2) > 1) {
            return 1;
        }
        return 0;
    }
};

class TicTacToe : public GameObject {
public:
    TicTacToe() = default;

    bool start() {
        players_[0] = std::make_unique<HumanPlayer>(Field::TOKEN_PLAYER_A, "Player A");
        players_[1] = std::make_unique<ArtificialPlayer>(Field::TOKEN_PLAYER_B, "Player B");
        return players_[0] != nullptr && players_[1] != nullptr;
    }

    bool check_winner(const Player& player) {
        return field_.same_in_row(player.token(), 3) > 0;
    }

    bool is_draw() {
        return field_.is_draw();
    }

    void run() {
        int player_index = 0;
        for (int turn = 0; turn < 9; ++turn) {
            Player& player = *players_[player_index];
            const Field::Move move = player.turn(field_);
            field_.make_move(move, player.token());
            check_winner(player);
            is_draw();
            if (check_winner(player)) {
                announce();
                std::cout << player.name() << " won\n";
                return;
            }
            if (is_draw()) {
                announce();
                std::cout << "Draw\n";
                return;
            }
            player_index = (player_index + 1) % 2;
        }
    }

private:
    Field field_;
    std::array<std::unique_ptr<Player>, 2> players_{};
};

void probe_check_winner(TicTacToe& game) {
    game.check_winner(
        HumanPlayer(Field::TOKEN_PLAYER_A, "Probe")
    );
}

int main() {
    TicTacToe game;
    probe_check_winner(game);
    if (game.start()) {
        game.run();
    }
    return 0;
}
