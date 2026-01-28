(function_declaration
  name: (identifier) @name) @fun
{
  node @fun.node
  attr (@fun.node) kind = "FUNCTION"
  attr (@fun.node) name = (source-text @name)
}
