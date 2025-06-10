.PHONY: all build test clean

DB_URL := $(shell grep APP_database_url .env | cut -d= -f2)

migrate:
	sea-orm-cli migrate -u $(DB_URL) -s public

generate-entities:
	sea-orm-cli generate entity -u $(DB_URL) -o entity/src/entities

generate-migrations:
	sea-orm-cli migrate generate $(TARGET)