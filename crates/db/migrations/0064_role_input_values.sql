-- Store user-supplied input values for roles (separate from the schema in frontmatter).
ALTER TABLE roles ADD COLUMN input_values TEXT NOT NULL DEFAULT '{}';