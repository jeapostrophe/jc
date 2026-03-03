(internal_module
  "namespace" @context
  name: (_) @name) @item

(enum_declaration
  "enum" @context
  name: (_) @name) @item

(function_declaration
  "async"? @context
  "function" @context
  name: (_) @name
  parameters: (formal_parameters
    "(" @context
    ")" @context)) @item

(generator_function_declaration
  "async"? @context
  "function" @context
  "*" @context
  name: (_) @name
  parameters: (formal_parameters
    "(" @context
    ")" @context)) @item

(interface_declaration
  "interface" @context
  name: (_) @name) @item

(program
  (export_statement
    (lexical_declaration
      [
        "let"
        "const"
      ] @context
      (variable_declarator
        name: (identifier) @name) @item)))

(program
  (lexical_declaration
    [
      "let"
      "const"
    ] @context
    (variable_declarator
      name: (identifier) @name) @item))

(class_declaration
  "class" @context
  name: (_) @name) @item

(class_body
  (method_definition
    [
      "get"
      "set"
      "async"
      "*"
      "readonly"
      "static"
      (override_modifier)
      (accessibility_modifier)
    ]* @context
    name: (_) @name
    parameters: (formal_parameters
      "(" @context
      ")" @context)) @item)

(public_field_definition
  [
    "declare"
    "readonly"
    "abstract"
    "static"
    (accessibility_modifier)
  ]* @context
  name: (_) @name) @item

(comment) @annotation
