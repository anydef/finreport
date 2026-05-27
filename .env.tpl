DOCKER_REGISTRY="op://HomeLab/DockerRegistry/hostname"
PORTAINER_URL="op://HomeLab/Portainer Tower/host"
PORTAINER_ACCESS_TOKEN="op://HomeLab/Portainer Tower/access_token"
GARAGE_ADMIN_TOKEN="op://HomeLab/Garage-API/admin_token"
AWS_ACCESS_KEY_ID="op://HomeLab/Garage-API/API Keys/id"
AWS_SECRET_ACCESS_KEY="op://HomeLab/Garage-API/API Keys/secret"
TF_VAR_opnsense_api_key="op://HomeLab/OPNSense Admin/key"
TF_VAR_opnsense_api_secret="op://HomeLab/OPNSense Admin/secret"
TF_VAR_opnsense_url="op://HomeLab/OPNSense Admin/hostname"

# Comdirect API credentials.
# `APP_*` is consumed by `just import-local` (cargo run from the host).
# `TF_VAR_app_*` is consumed by terraform and injected into the Portainer stack
# so the deployed containers see them as `APP_*` at runtime.
APP_client_id="op://HomeLab/finreport/comdirect/client_id"
APP_client_secret="op://HomeLab/finreport/comdirect/client_secret"
APP_zugangsnummer="op://HomeLab/finreport/comdirect/zugangsnummer"
APP_pin="op://HomeLab/finreport/comdirect/pin"
TF_VAR_app_client_id="op://HomeLab/finreport/comdirect/client_id"
TF_VAR_app_client_secret="op://HomeLab/finreport/comdirect/client_secret"
TF_VAR_app_zugangsnummer="op://HomeLab/finreport/comdirect/zugangsnummer"
TF_VAR_app_pin="op://HomeLab/finreport/comdirect/pin"

# Postgres password — pulled from 1Password and passed to Terraform, which
# then injects it into the Portainer stack via extra_env.
TF_VAR_postgres_password="op://HomeLab/finreport/psql/password"