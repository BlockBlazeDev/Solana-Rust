#!/bin/bash -ex

cd "$(dirname "$0")"/../..

deployMethod="$1"
nodeType="$2"
publicNetwork="$3"
entrypointIp="$4"
numNodes="$5"
RUST_LOG="$6"

[[ -n $deployMethod ]] || exit
[[ -n $nodeType ]] || exit
[[ -n $publicNetwork ]] || exit
[[ -n $entrypointIp ]] || exit
[[ -n $numNodes ]] || exit

cat > deployConfig <<EOF
deployMethod="$deployMethod"
entrypointIp="$entrypointIp"
numNodes="$numNodes"
EOF

source net/common.sh
loadConfigFile

scripts/install-earlyoom.sh

if [[ $publicNetwork = true ]]; then
  setupArgs="-p"
else
  setupArgs="-l"
fi


case $deployMethod in
snap)
  SECONDS=0
  rsync -vPrc "$entrypointIp:~/solana/solana.snap" .
  sudo snap install solana.snap --devmode --dangerous

  commonNodeConfig="\
    leader-ip=$entrypointIp \
    default-metrics-rate=1 \
    metrics-config=$SOLANA_METRICS_CONFIG \
    rust-log=$RUST_LOG \
    setup-args=$setupArgs \
  "

  if [[ -e /dev/nvidia0 ]]; then
    commonNodeConfig="$commonNodeConfig enable-cuda=1"
  fi

  if [[ $nodeType = leader ]]; then
    nodeConfig="mode=leader+drone $commonNodeConfig"
    ln -sf -T /var/snap/solana/current/leader/current leader.log
    ln -sf -T /var/snap/solana/current/drone/current drone.log
  else
    nodeConfig="mode=validator $commonNodeConfig"
    ln -sf -T /var/snap/solana/current/validator/current validator.log
  fi

  logmarker="solana deploy $(date)/$RANDOM"
  logger "$logmarker"

  # shellcheck disable=SC2086 # Don't want to double quote "$nodeConfig"
  sudo snap set solana $nodeConfig
  snap info solana
  sudo snap get solana
  echo Slight delay to get more syslog output
  sleep 2
  sudo grep -Pzo "$logmarker(.|\\n)*" /var/log/syslog

  echo "Succeeded in ${SECONDS} seconds"
  ;;
local)
  PATH="$HOME"/.cargo/bin:"$PATH"
  export USE_INSTALL=1
  export RUST_LOG
  export SOLANA_DEFAULT_METRICS_RATE=1

  ./fetch-perf-libs.sh
  export LD_LIBRARY_PATH="$PWD/target/perf-libs:$LD_LIBRARY_PATH"

  scripts/oom-monitor.sh  > oom-monitor.log 2>&1 &

  case $nodeType in
  leader)
    ./multinode-demo/setup.sh -t leader $setupArgs
    ./multinode-demo/drone.sh > drone.log 2>&1 &
    ./multinode-demo/leader.sh > leader.log 2>&1 &
    ;;
  validator)
    rsync -vPrc "$entrypointIp:~/.cargo/bin/solana*" ~/.cargo/bin/

    ./multinode-demo/setup.sh -t validator $setupArgs
    ./multinode-demo/validator.sh "$entrypointIp":~/solana "$entrypointIp:8001" >validator.log 2>&1 &
    ;;
  *)
    echo "Error: unknown node type: $nodeType"
    exit 1
    ;;
  esac
  ;;
*)
  echo "Unknown deployment method: $deployMethod"
  exit 1
esac
