# Local development helpers. `make run-local` runs the service against a Postgres
# you already have running; see the README section
# "Using an existing Postgres instead of Docker".

# Connection string for your local Postgres. Override per invocation or from the
# environment, e.g.
#   make run-local DATABASE_URL='postgres://me:secret@localhost:5432/master_data'
DATABASE_URL ?= postgres://postgres:postgres@localhost:5432/master_data

# HS256 signing secret for tokens you mint locally. Dev default; it must match
# whatever signed the token you send.
APP_AUTH__JWT_SECRET ?= dev-secret

# Environment for `cargo run`. SQLX_OFFLINE uses the committed .sqlx cache, so the
# build needs no live schema; the service applies migrations at startup.
LOCAL_ENV = SQLX_OFFLINE=true DATABASE_URL='$(DATABASE_URL)' APP_DATABASE__URL='$(DATABASE_URL)' APP_AUTH__JWT_SECRET='$(APP_AUTH__JWT_SECRET)'

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  %-12s %s\n", $$1, $$2}'

.PHONY: run-local
run-local: ## Run the service against an existing local Postgres
	@echo "database : $(DATABASE_URL)"
	@echo "docs     : http://localhost:8080/docs  (Swagger UI)"
	@echo "One URL sets both DATABASE_URL (sqlx) and APP_DATABASE__URL (the service)."
	@$(LOCAL_ENV) cargo run

.PHONY: db-create
db-create: ## Create the database named in DATABASE_URL (errors if it already exists)
	createdb "$(DATABASE_URL)"

.PHONY: migrate
migrate: ## Apply migrations now instead of waiting for server startup
	DATABASE_URL='$(DATABASE_URL)' sqlx migrate run
