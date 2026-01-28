(method_declaration
  name: (identifier) @name) @method
{
  node @method.node
  attr (@method.node) kind = "METHOD"
  attr (@method.node) name = (source-text @name)
}

(class_declaration
  name: (identifier) @name) @class
{
  node @class.node
  attr (@class.node) kind = "CLASS"
  attr (@class.node) name = (source-text @name)
}
