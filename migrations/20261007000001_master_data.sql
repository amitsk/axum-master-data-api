CREATE TYPE entity_type AS ENUM (
  'country','currency','language','category','gender',
  'age','geography','product_line','business_unit','product_tier'
);

CREATE FUNCTION jsonb_key_count(j jsonb) RETURNS integer
  LANGUAGE sql IMMUTABLE AS
  $$ SELECT count(*)::integer FROM jsonb_object_keys(j) $$;

CREATE TABLE master_data (
  entity_type  entity_type NOT NULL,
  id           BIGINT      NOT NULL,
  code         TEXT        NOT NULL,
  name         TEXT        NOT NULL,
  short_name   TEXT        NOT NULL,
  attributes   JSONB       NOT NULL DEFAULT '{}'::jsonb,
  version      INTEGER     NOT NULL DEFAULT 1,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by   TEXT        NOT NULL,
  deleted_at   TIMESTAMPTZ,
  PRIMARY KEY (entity_type, id),
  CONSTRAINT attributes_is_object CHECK (jsonb_typeof(attributes) = 'object'),
  CONSTRAINT attributes_max_two   CHECK (jsonb_key_count(attributes) <= 2)
);

CREATE UNIQUE INDEX master_data_code_live
  ON master_data (entity_type, code) WHERE deleted_at IS NULL;

CREATE INDEX master_data_name_live
  ON master_data (entity_type, lower(name)) WHERE deleted_at IS NULL;

CREATE TABLE id_counters (
  entity_type entity_type PRIMARY KEY,
  next_id     BIGINT NOT NULL DEFAULT 1
);

INSERT INTO id_counters (entity_type)
SELECT unnest(enum_range(NULL::entity_type));