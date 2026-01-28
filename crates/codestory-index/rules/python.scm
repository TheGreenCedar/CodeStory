(function_definition
  name: (identifier) @name) @fun
{
  node @fun.node
  attr (@fun.node) kind = "FUNCTION"
  attr (@fun.node) name = (source-text @name)
}

(class_definition
  name: (identifier) @name) @class
{
  node @class.node
  attr (@class.node) kind = "CLASS"
  attr (@class.node) name = (source-text @name)
}

;; Membership
(class_definition
  body: (block
    (function_definition
      name: (identifier)) @method)) @class_def
{
  edge @class_def.node -> @method.node
  attr (edge @class_def.node -> @method.node) kind = "MEMBER"
}

;; Inheritance
(class_definition
  superclasses: (argument_list
    (identifier) @parent_name)) @class_def
{
  node @parent_name.node
  attr (@parent_name.node) kind = "CLASS"
  attr (@parent_name.node) name = (source-text @parent_name)

  edge @class_def.node -> @parent_name.node
  attr (edge @class_def.node -> @parent_name.node) kind = "INHERITANCE"
}

;; Function Calls (Simple)
(call
  function: (identifier) @callee_name) @call_site
{
  ;; We can't easily get the enclosing function scope node in a flat rule,
  ;; but we can at least ensure the callee is indexed as a reference.
  node @callee_name.node
  attr (@callee_name.node) kind = "FUNCTION"
  attr (@callee_name.node) name = (source-text @callee_name)
}
