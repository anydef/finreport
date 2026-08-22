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
# `__name` is a plain human-readable label (no secret), stored on every account
# row the login imports so they can be told apart in the UI. It is display only:
# a login is addressed by its key (`--account 0`), never by name, so renaming
# one here is safe.
#
# `APP_accounts__*` is consumed by `just import-local` (cargo run from the host).
APP_accounts__0__name="Comdirect Family"
APP_accounts__0__client_id="op://HomeLab/finreport/comdirect/client_id"
APP_accounts__0__client_secret="op://HomeLab/finreport/comdirect/client_secret"
APP_accounts__0__zugangsnummer="op://HomeLab/finreport/comdirect/zugangsnummer"
APP_accounts__0__pin="op://HomeLab/finreport/comdirect/pin"

# Second login — repoint these at its own 1Password item and uncomment, then
# add it to TF_VAR_app_comdirect_accounts below.
#APP_accounts__1__name="Comdirect Pavlo"
#APP_accounts__1__client_id="op://HomeLab/finreport/comdirect 42992464/client_id"
#APP_accounts__1__client_secret="op://HomeLab/finreport/comdirect 42992464/client_secret"
#APP_accounts__1__zugangsnummer="op://HomeLab/finreport/comdirect 42992464/zugangsnummer"
#APP_accounts__1__pin="op://HomeLab/finreport/comdirect 42992464/pin"

# Terraform takes the same logins as one list(object) variable, which has to be
# a JSON string. The deploy script evals this file, so it is assembled from the
# already-resolved vars above rather than repeating the op:// references — keep
# the array in importer order, element 0 first.
TF_VAR_app_comdirect_accounts="[{\"name\":\"${APP_accounts__0__name}\",\"client_id\":\"${APP_accounts__0__client_id}\",\"client_secret\":\"${APP_accounts__0__client_secret}\",\"zugangsnummer\":\"${APP_accounts__0__zugangsnummer}\",\"pin\":\"${APP_accounts__0__pin}\"}]"

# Postgres password — pulled from 1Password and passed to Terraform, which
# then injects it into the Portainer stack via extra_env.
TF_VAR_postgres_password="op://HomeLab/finreport/psql/password"