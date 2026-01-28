(function_item
  name: (identifier) @name) @fun
{
  node @fun.node
  attr (@fun.node) kind = "FUNCTION"
  attr (@fun.node) name = (source-text @name)
}

(struct_item
  name: (type_identifier) @name) @struct
{
  node @struct.node
  attr (@struct.node) kind = "STRUCT"
  attr (@struct.node) name = (source-text @name)
}

(field_declaration
  name: (field_identifier) @name) @field
{
  node @field.node
  attr (@field.node) kind = "FIELD"
  attr (@field.node) name = (source-text @name)
}

(struct_item
  (field_declaration_list
    (field_declaration) @field
  )
) @struct
{
  edge @struct.node -> @field.node
  attr (edge @struct.node -> @field.node) kind = "MEMBER"
}