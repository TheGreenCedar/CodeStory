(method_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "METHOD"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(class_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(interface_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "INTERFACE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(struct_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "STRUCT"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(namespace_declaration
  name: [(identifier) (qualified_name)] @name) @def
{
  node @name.node
  attr (@name.node) kind = "NAMESPACE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(file_scoped_namespace_declaration
  name: [(identifier) (qualified_name)] @name) @def
{
  node @name.node
  attr (@name.node) kind = "NAMESPACE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

;; Namespace membership
(compilation_unit
  (file_scoped_namespace_declaration name: [(identifier) (qualified_name)] @namespace_name)
  (class_declaration name: (identifier) @class_name))
{
  edge @namespace_name.node -> @class_name.node
  attr (@namespace_name.node -> @class_name.node) kind = "MEMBER"
}

(compilation_unit
  (file_scoped_namespace_declaration name: [(identifier) (qualified_name)] @namespace_name)
  (interface_declaration name: (identifier) @class_name))
{
  edge @namespace_name.node -> @class_name.node
  attr (@namespace_name.node -> @class_name.node) kind = "MEMBER"
}

(compilation_unit
  (file_scoped_namespace_declaration name: [(identifier) (qualified_name)] @namespace_name)
  (struct_declaration name: (identifier) @class_name))
{
  edge @namespace_name.node -> @class_name.node
  attr (@namespace_name.node -> @class_name.node) kind = "MEMBER"
}

(namespace_declaration
  name: [(identifier) (qualified_name)] @namespace_name
  body: (declaration_list
    (class_declaration name: (identifier) @class_name)))
{
  edge @namespace_name.node -> @class_name.node
  attr (@namespace_name.node -> @class_name.node) kind = "MEMBER"
}

(namespace_declaration
  name: [(identifier) (qualified_name)] @namespace_name
  body: (declaration_list
    (interface_declaration name: (identifier) @class_name)))
{
  edge @namespace_name.node -> @class_name.node
  attr (@namespace_name.node -> @class_name.node) kind = "MEMBER"
}

(namespace_declaration
  name: [(identifier) (qualified_name)] @namespace_name
  body: (declaration_list
    (struct_declaration name: (identifier) @class_name)))
{
  edge @namespace_name.node -> @class_name.node
  attr (@namespace_name.node -> @class_name.node) kind = "MEMBER"
}

;; Inheritance
(class_declaration
  name: (identifier) @class_name
  (base_list (_) @parent_name))
{
  node @parent_name.node
  attr (@parent_name.node) kind = "CLASS"
  attr (@parent_name.node) name = (source-text @parent_name)
  attr (@parent_name.node) start_row = (start-row @parent_name)
  attr (@parent_name.node) start_col = (start-column @parent_name)
  attr (@parent_name.node) end_row = (end-row @parent_name)
  attr (@parent_name.node) end_col = (end-column @parent_name)

  edge @class_name.node -> @parent_name.node
  attr (@class_name.node -> @parent_name.node) kind = "INHERITANCE"
}

;; Calls
(invocation_expression
  function: (identifier) @callee_any) @call_any
{
  node @call_any.node
  attr (@call_any.node) kind = "UNKNOWN"
  attr (@call_any.node) name = (source-text @callee_any)
  attr (@call_any.node) start_row = (start-row @callee_any)
  attr (@call_any.node) start_col = (start-column @callee_any)
  attr (@call_any.node) end_row = (end-row @callee_any)
  attr (@call_any.node) end_col = (end-column @callee_any)

  edge @call_any.node -> @call_any.node
  attr (@call_any.node -> @call_any.node) kind = "CALL"
  attr (@call_any.node -> @call_any.node) line = (start-row @call_any)
}

(invocation_expression
  function: (member_access_expression
    name: (identifier) @callee_any) @call_any)
{
  node @call_any.node
  attr (@call_any.node) kind = "UNKNOWN"
  attr (@call_any.node) name = (source-text @callee_any)
  attr (@call_any.node) start_row = (start-row @callee_any)
  attr (@call_any.node) start_col = (start-column @callee_any)
  attr (@call_any.node) end_row = (end-row @callee_any)
  attr (@call_any.node) end_col = (end-column @callee_any)

  edge @call_any.node -> @call_any.node
  attr (@call_any.node -> @call_any.node) kind = "CALL"
  attr (@call_any.node -> @call_any.node) line = (start-row @call_any)
  attr (@call_any.node -> @call_any.node) call_syntax = "csharp_member"
}

;; Using directives
(using_directive
  (identifier) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}

(using_directive
  (qualified_name) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}
