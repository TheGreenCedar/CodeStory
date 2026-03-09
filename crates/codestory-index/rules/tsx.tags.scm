(class_declaration
  name: (type_identifier) @definition.class)

(interface_declaration
  name: (type_identifier) @definition.interface)

(type_alias_declaration
  name: (type_identifier) @definition.typedef)

(enum_declaration
  name: (identifier) @definition.enum)

(function_declaration
  name: (identifier) @definition.function)

(method_definition
  (accessibility_modifier) @access
  name: (_) @definition.method)

(method_definition
  name: (_) @definition.method)

(method_signature
  (accessibility_modifier) @access
  name: (_) @definition.method)

(method_signature
  name: (_) @definition.method)

(public_field_definition
  (accessibility_modifier) @access
  name: (_) @definition.field)

(public_field_definition
  name: (_) @definition.field)

(property_signature
  name: (_) @definition.field)
