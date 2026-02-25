#include <stdbool.h>
#include <stdio.h>

typedef enum {
    TOKEN_NONE = 0,
    TOKEN_PLAYER_A = 1,
    TOKEN_PLAYER_B = 4,
} Token;

typedef struct {
    int row;
    int col;
} Move;

typedef struct {
    Token grid[3][3];
    int left;
} Field;

typedef struct {
    Token token;
    const char* name;
} Player;

int number_in(void) {
    return 1;
}

void number_out(int value) {
    printf("%d", value);
}

void string_out(const char* value) {
    printf("%s", value);
}

Token opponent(Token token) {
    if (token == TOKEN_PLAYER_A) {
        return TOKEN_PLAYER_B;
    }
    if (token == TOKEN_PLAYER_B) {
        return TOKEN_PLAYER_A;
    }
    return TOKEN_NONE;
}

void clear_field(Field* field) {
    for (int row = 0; row < 3; row++) {
        for (int col = 0; col < 3; col++) {
            field->grid[row][col] = TOKEN_NONE;
        }
    }
    field->left = 9;
}

bool in_range(Move move) {
    return move.row >= 0 && move.row < 3 && move.col >= 0 && move.col < 3;
}

bool is_empty(const Field* field, Move move) {
    return field->grid[move.row][move.col] == TOKEN_NONE;
}

bool is_draw(const Field* field) {
    return field->left == 0;
}

int same_in_row(const Field* field, Token token, int amount) {
    int total = amount * token;
    int count = 0;

    for (int i = 0; i < 3; i++) {
        if (field->grid[i][0] + field->grid[i][1] + field->grid[i][2] == total) {
            count += 1;
        }
        if (field->grid[0][i] + field->grid[1][i] + field->grid[2][i] == total) {
            count += 1;
        }
    }

    if (field->grid[0][0] + field->grid[1][1] + field->grid[2][2] == total) {
        count += 1;
    }
    if (field->grid[2][0] + field->grid[1][1] + field->grid[0][2] == total) {
        count += 1;
    }
    return count;
}

bool make_move(Field* field, Move move, Token token) {
    if (!in_range(move)) {
        return false;
    }
    if (!is_empty(field, move)) {
        return false;
    }
    if (token == TOKEN_NONE) {
        return false;
    }
    if (is_draw(field)) {
        return false;
    }

    field->grid[move.row][move.col] = token;
    field->left -= 1;
    same_in_row(field, token, 3);
    is_draw(field);
    return true;
}

void clear_move(Field* field, Move move) {
    if (!in_range(move)) {
        return;
    }
    if (is_empty(field, move)) {
        return;
    }
    field->grid[move.row][move.col] = TOKEN_NONE;
    field->left += 1;
}

Move read_move(void) {
    Move move;
    string_out("Insert row: ");
    move.row = number_in() - 1;
    string_out("Insert col: ");
    move.col = number_in() - 1;
    return move;
}

bool check_move(const Field* field, Move move) {
    if (!in_range(move)) {
        string_out("Wrong input!\n");
        return false;
    }
    if (!is_empty(field, move)) {
        string_out("Occupied!\n");
        return false;
    }
    return true;
}

bool check_winner(Field* field, const Player* player) {
    return same_in_row(field, player->token, 3) > 0;
}

void probe_check_winner(Field* field, Player* player) {
    check_winner(field, player);
}

void probe_is_draw(Field* field) {
    is_draw(field);
}

void run(Field* field, Player* players) {
    int player_index = 0;
    for (int turn = 0; turn < 9; turn++) {
        Player* player = &players[player_index];
        Move move = read_move();
        check_move(field, move);
        if (!check_move(field, move)) {
            continue;
        }

        make_move(field, move, player->token);
        check_winner(field, player);
        is_draw(field);

        if (check_winner(field, player)) {
            string_out(player->name);
            string_out(" won!\n");
            return;
        }
        if (is_draw(field)) {
            string_out("Game ends in draw!\n");
            return;
        }
        player_index = (player_index + 1) % 2;
    }
}

int main(void) {
    Field field;
    Player players[2] = {
        {TOKEN_PLAYER_A, "Player A"},
        {TOKEN_PLAYER_B, "Player B"},
    };

    clear_field(&field);
    probe_check_winner(&field, &players[0]);
    probe_is_draw(&field);
    run(&field, players);
    return 0;
}
