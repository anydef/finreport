DOCKER_REGISTRY="op://HomeLab/DockerRegistry/hostname"
PORTAINER_URL="op://HomeLab/Portainer Tower/host"
PORTAINER_ACCESS_TOKEN="op://HomeLab/Portainer Tower/access_token"
TF_VAR_opnsense_api_key="op://HomeLab/OPNSense Admin/key"
TF_VAR_opnsense_api_secret="op://HomeLab/OPNSense Admin/secret"
TF_VAR_opnsense_url="op://HomeLab/OPNSense Admin/hostname"

# Comdirect API credentials (consumed by webapp via APP_* env prefix)
APP_client_id="op://HomeLab/finreport/comdirect/client_id"
APP_client_secret="op://HomeLab/finreport/comdirect/client_secret"
APP_zugangsnummer="op://HomeLab/finreport/comdirect/zugangsnummer"
APP_pin="op://HomeLab/finreport/comdirect/pin"

# Postgres password — pulled from 1Password and passed to Terraform, which
# then injects it into the Portainer stack via extra_env.
TF_VAR_postgres_password="op://HomeLab/finreport/psql/password"