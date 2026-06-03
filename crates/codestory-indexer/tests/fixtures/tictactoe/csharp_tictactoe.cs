using System;

namespace TicTacToe;

static class Entry
{
    public static int numberIn() => 1;

    public static void numberOut(int num) => Console.Write(num);

    public static void stringOut(string value) => Console.Write(value);
}

class GameObject
{
    public void announce() { }
}

class Field : GameObject
{
    public int[,] grid = new int[3, 3];
    public int left = 9;

    public int opponent(int token)
    {
        if (token == 1) return 4;
        if (token == 4) return 1;
        return 0;
    }

    public int sameInRow(int token, int amount)
    {
        int total = token * amount;
        int count = 0;
        for (int i = 0; i < 3; i++)
        {
            if (grid[i, 0] + grid[i, 1] + grid[i, 2] == total) count++;
        }
        return count;
    }

    public bool makeMove(int row, int col, int token)
    {
        if (token == 0) return false;
        grid[row, col] = token;
        left--;
        sameInRow(token, 3);
        return true;
    }
}

interface Player
{
    bool turn(Field field, int token);
}

class HumanPlayer : Player
{
    public bool turn(Field field, int token) => field.makeMove(0, 0, token);
}

class ArtificialPlayer : Player
{
    public int minMax(Field field, int token, int depth)
    {
        if (depth == 0) return 0;
        return minMax(field, token, depth - 1);
    }

    public bool turn(Field field, int token)
    {
        minMax(field, token, 3);
        return true;
    }
}

class TicTacToe : GameObject
{
    public Field field = new();

    public void run()
    {
        numberIn();
        stringOut("start");
        new Random().Next(3);
    }
}

class Program
{
    static void Main()
    {
        new TicTacToe().run();
    }
}
