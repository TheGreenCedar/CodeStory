<?php

namespace TicTacToe;

use Random\Randomizer;

function numberIn(): int
{
    return 1;
}

function numberOut(int $num): void
{
    echo $num;
}

function stringOut(string $value): void
{
    echo $value;
}

class GameObject
{
    public function announce(): void
    {
    }
}

class Field extends GameObject
{
    /** @var array<int, array<int, int>> */
    public array $grid;
    public int $left = 9;

    public function __construct()
    {
        $this->grid = array_fill(0, 3, array_fill(0, 3, 0));
    }

    public function opponent(int $token): int
    {
        if ($token === 1) {
            return 4;
        }
        if ($token === 4) {
            return 1;
        }
        return 0;
    }

    public function sameInRow(int $token, int $amount): int
    {
        $total = $token * $amount;
        $count = 0;
        for ($i = 0; $i < 3; $i++) {
            if ($this->grid[$i][0] + $this->grid[$i][1] + $this->grid[$i][2] === $total) {
                $count++;
            }
        }
        return $count;
    }

    public function makeMove(int $row, int $col, int $token): bool
    {
        if ($token === 0) {
            return false;
        }
        $this->grid[$row][$col] = $token;
        $this->left--;
        $this->sameInRow($token, 3);
        return true;
    }
}

interface Player
{
    public function turn(Field $field, int $token): bool;
}

class HumanPlayer implements Player
{
    public function turn(Field $field, int $token): bool
    {
        return $field->makeMove(0, 0, $token);
    }
}

class ArtificialPlayer implements Player
{
    public function minMax(Field $field, int $token, int $depth): int
    {
        if ($depth === 0) {
            return 0;
        }
        return $this->minMax($field, $token, $depth - 1);
    }

    public function turn(Field $field, int $token): bool
    {
        $this->minMax($field, $token, 3);
        return true;
    }
}

class TicTacToe extends GameObject
{
    public Field $field;

    public function run(): void
    {
        $this->field = new Field();
        numberIn();
        stringOut("start");
        (new Randomizer())->nextInt(0, 2);
    }
}

(new TicTacToe())->run();
