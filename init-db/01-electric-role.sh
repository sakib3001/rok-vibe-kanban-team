#!/bin/sh
# Runs once on first Postgres init (empty data dir). Creates the electric_sync
# REPLICATION role that ElectricSQL connects as, and grants it read access.
# Mirrors the BYO-Postgres setup documented in the Helm chart README.
set -e

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    -- ElectricSQL replication role
    CREATE ROLE electric_sync WITH LOGIN PASSWORD '${ELECTRIC_ROLE_PASSWORD}' REPLICATION;

    -- Read access to the app database and current/future tables
    GRANT ALL PRIVILEGES ON DATABASE "${POSTGRES_DB}" TO electric_sync;
    GRANT USAGE ON SCHEMA public TO electric_sync;
    GRANT SELECT ON ALL TABLES IN SCHEMA public TO electric_sync;

    -- Tables created later by the remote server (owned by ${POSTGRES_USER})
    -- will automatically grant SELECT to electric_sync:
    ALTER DEFAULT PRIVILEGES IN SCHEMA public
        GRANT SELECT ON TABLES TO electric_sync;
EOSQL

echo "electric_sync role created."
