# |source| this file
#
# Common utilities shared by other scripts in this directory
#
# The following directive disable complaints about unused variables in this
# file:
# shellcheck disable=2034
#

netDir=$(
  cd "$(dirname "${BASH_SOURCE[0]}")" || exit
  echo "$PWD"
)
netConfigDir="$netDir"/config
netLogDir="$netDir"/log
mkdir -p "$netConfigDir" "$netLogDir"

# shellcheck source=scripts/configure-metrics.sh
source "$(dirname "${BASH_SOURCE[0]}")"/../scripts/configure-metrics.sh

configFile="$netConfigDir/config"

entrypointIp=
publicNetwork=
netBasename=
sshPrivateKey=
sshOptions=()
fullnodeIpList=()
fullnodeIpListPrivate=()
clientIpList=()
clientIpListPrivate=()
apiIpList=()
apiIpListPrivate=()
leaderRotation=

buildSshOptions() {
  sshOptions=(
    -o "BatchMode=yes"
    -o "StrictHostKeyChecking=no"
    -o "UserKnownHostsFile=/dev/null"
    -o "User=solana"
    -o "IdentityFile=$sshPrivateKey"
    -o "LogLevel=ERROR"
    -F /dev/null
  )
}

loadConfigFile() {
  [[ -r $configFile ]] || usage "Config file unreadable: $configFile"

  # shellcheck source=/dev/null
  source "$configFile"
  [[ -n "$publicNetwork" ]] || usage "Config file invalid, publicNetwork unspecified: $configFile"
  [[ -n "$netBasename" ]] || usage "Config file invalid, netBasename unspecified: $configFile"
  [[ -n $sshPrivateKey ]] || usage "Config file invalid, sshPrivateKey unspecified: $configFile"
  [[ -n $leaderRotation ]] || usage "Config file invalid, leaderRotation unspecified: $configFile"
  [[ ${#fullnodeIpList[@]} -gt 0 ]] || usage "Config file invalid, fullnodeIpList unspecified: $configFile"
  [[ ${#fullnodeIpListPrivate[@]} -gt 0 ]] || usage "Config file invalid, fullnodeIpListPrivate unspecified: $configFile"
  [[ ${#fullnodeIpList[@]} -eq ${#fullnodeIpListPrivate[@]} ]] || usage "Config file invalid, fullnodeIpList/fullnodeIpListPrivate length mismatch: $configFile"

  if $publicNetwork; then
    entrypointIp=${fullnodeIpList[0]}
  else
    entrypointIp=${fullnodeIpListPrivate[0]}
  fi

  buildSshOptions
  configureMetrics
}
