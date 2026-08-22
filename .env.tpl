DOCKER_REGISTRY="op://HomeLab/DockerRegistry/hostname"
PORTAINER_URL="op://HomeLab/Portainer Tower/host"
PORTAINER_ACCESS_TOKEN="op://HomeLab/Portainer Tower/access_token"
GARAGE_ADMIN_TOKEN="op://HomeLab/Garage-API/admin_token"
AWS_ACCESS_KEY_ID="op://HomeLab/Garage-API/API Keys/id"
AWS_SECRET_ACCESS_KEY="op://HomeLab/Garage-API/API Keys/secret"
TF_VAR_opnsense_api_key="op://HomeLab/OPNSense Admin/key"
TF_VAR_opnsense_api_secret="op://HomeLab/OPNSense Admin/secret"
TF_VAR_opnsense_url="op://HomeLab/OPNSense Admin/hostname"

# Comdirect logins, one block per online-banking login, numbered. The importer
# imports them all in a single process (one task per login), so each approves
# its own push-TAN without blocking the others.
#
# Every value here must be exactly one op:// reference and nothing else — this
# file is read three different ways (op run for `just import-local`, op inject
# for a local `just deploy`, and 1password/load-secrets-action in CI) and only
# the plain one-reference-per-line form works in all of them. In particular
# nothing here is shell-expanded in CI, so no value may refer to another.
#
# APP_accounts__* is consumed by `just import-local` (cargo run from the host);
# TF_VAR_app_account_* is consumed by terraform, which flattens the accounts
# into the APP_accounts__<n>__* form the deployed container reads.
#
# The human-readable label is not a secret and lives in docker-compose.yml
# (APP_accounts__0__name), not here.
APP_accounts__0__client_id="op://HomeLab/finreport/comdirect/client_id"
APP_accounts__0__client_secret="op://HomeLab/finreport/comdirect/client_secret"
APP_accounts__0__zugangsnummer="op://HomeLab/finreport/comdirect/zugangsnummer"
APP_accounts__0__pin="op://HomeLab/finreport/comdirect/pin"
TF_VAR_app_account_0_client_id="op://HomeLab/finreport/comdirect/client_id"
TF_VAR_app_account_0_client_secret="op://HomeLab/finreport/comdirect/client_secret"
TF_VAR_app_account_0_zugangsnummer="op://HomeLab/finreport/comdirect/zugangsnummer"
TF_VAR_app_account_0_pin="op://HomeLab/finreport/comdirect/pin"

# Second login, from its own 1Password item.
APP_accounts__1__client_id="op://HomeLab/finreport/comdirect 42992464/client_id"
APP_accounts__1__client_secret="op://HomeLab/finreport/comdirect 42992464/client_secret"
APP_accounts__1__zugangsnummer="op://HomeLab/finreport/comdirect 42992464/zugangsnummer"
APP_accounts__1__pin="op://HomeLab/finreport/comdirect 42992464/pin"
TF_VAR_app_account_1_client_id="op://HomeLab/finreport/comdirect 42992464/client_id"
TF_VAR_app_account_1_client_secret="op://HomeLab/finreport/comdirect 42992464/client_secret"
TF_VAR_app_account_1_zugangsnummer="op://HomeLab/finreport/comdirect 42992464/zugangsnummer"
TF_VAR_app_account_1_pin="op://HomeLab/finreport/comdirect 42992464/pin"

# Postgres password — pulled from 1Password and passed to Terraform, which
# then injects it into the Portainer stack via extra_env.
TF_VAR_postgres_password="op://HomeLab/finreport/psql/password"