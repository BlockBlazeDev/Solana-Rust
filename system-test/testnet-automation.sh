#!/usr/bin/env bash
set -e

# shellcheck disable=SC1090
# shellcheck disable=SC1091
source "$(dirname "$0")"/automation_utils.sh

function cleanup_testnet {
  RC=$?
  if [[ $RC != 0 ]]; then
    RESULT_DETAILS="
Test failed during step:
${STEP}

Failure occured when running the following command:
$(eval echo "$@")"
  fi

# shellcheck disable=SC2034
  TESTNET_FINISH_UNIX_MSECS="$(($(date +%s%N)/1000000))"
  if [[ "$UPLOAD_RESULTS_TO_SLACK" = "true" ]]; then
    upload_results_to_slack
  fi

  (
    set +e
    execution_step "Collecting Logfiles from Nodes"
    collect_logs
  )

  (
    set +e
    execution_step "Stop Network Software"
    "${REPO_ROOT}"/net/net.sh stop
  )

  (
    set +e
    analyze_packet_loss
  )

  execution_step "Deleting Testnet"
  "${REPO_ROOT}"/net/"${CLOUD_PROVIDER}".sh delete -p "${TESTNET_TAG}"

}
trap 'cleanup_testnet $BASH_COMMAND' EXIT

function launch_testnet() {
  set -x

  # shellcheck disable=SC2068
  execution_step "Create ${NUMBER_OF_VALIDATOR_NODES} ${CLOUD_PROVIDER} nodes"

  case $CLOUD_PROVIDER in
    gce)
      if [[ -z $VALIDATOR_NODE_MACHINE_TYPE ]]; then
        echo VALIDATOR_NODE_MACHINE_TYPE not defined
        exit 1
      fi
    # shellcheck disable=SC2068
    # shellcheck disable=SC2086
      "${REPO_ROOT}"/net/gce.sh create \
        -d pd-ssd \
        -n "$NUMBER_OF_VALIDATOR_NODES" -c "$NUMBER_OF_CLIENT_NODES" \
        $maybeCustomMachineType "$VALIDATOR_NODE_MACHINE_TYPE" $maybeEnableGpu \
        -p "$TESTNET_TAG" $maybeCreateAllowBootFailures $maybePublicIpAddresses \
        ${TESTNET_CLOUD_ZONES[@]/#/"-z "} \
        --self-destruct-hours 0 \
        ${ADDITIONAL_FLAGS[@]/#/" "}
      ;;
    ec2)
    # shellcheck disable=SC2068
    # shellcheck disable=SC2086
      "${REPO_ROOT}"/net/ec2.sh create \
        -n "$NUMBER_OF_VALIDATOR_NODES" -c "$NUMBER_OF_CLIENT_NODES" \
        $maybeCustomMachineType "$VALIDATOR_NODE_MACHINE_TYPE" $maybeEnableGpu \
        -p "$TESTNET_TAG" $maybeCreateAllowBootFailures $maybePublicIpAddresses \
        ${TESTNET_CLOUD_ZONES[@]/#/"-z "} \
        ${ADDITIONAL_FLAGS[@]/#/" "}
      ;;
    azure)
    # shellcheck disable=SC2068
    # shellcheck disable=SC2086
      "${REPO_ROOT}"/net/azure.sh create \
        -n "$NUMBER_OF_VALIDATOR_NODES" -c "$NUMBER_OF_CLIENT_NODES" \
        $maybeCustomMachineType "$VALIDATOR_NODE_MACHINE_TYPE" $maybeEnableGpu \
        -p "$TESTNET_TAG" $maybeCreateAllowBootFailures $maybePublicIpAddresses \
        ${TESTNET_CLOUD_ZONES[@]/#/"-z "} \
        ${ADDITIONAL_FLAGS[@]/#/" "}
      ;;
    colo)
      "${REPO_ROOT}"/net/colo.sh delete --reclaim-preemptible-reservations
    # shellcheck disable=SC2068
    # shellcheck disable=SC2086
      "${REPO_ROOT}"/net/colo.sh create \
        -n "$NUMBER_OF_VALIDATOR_NODES" -c "$NUMBER_OF_CLIENT_NODES" $maybeEnableGpu \
        -p "$TESTNET_TAG" $maybePublicIpAddresses --dedicated \
        ${ADDITIONAL_FLAGS[@]/#/" "}
      ;;
    *)
      echo "Error: Unsupported cloud provider: $CLOUD_PROVIDER"
      ;;
    esac

  execution_step "Configure database"
  "${REPO_ROOT}"/net/init-metrics.sh -e

  execution_step "Fetch reusable testnet keypairs"
  if [[ ! -d "${REPO_ROOT}"/net/keypairs ]]; then
    git clone git@github.com:solana-labs/testnet-keypairs.git "${REPO_ROOT}"/net/keypairs
    # If we have provider-specific keys (CoLo*, GCE*, etc) use them instead of generic val*
    if [[ -d "${REPO_ROOT}"/net/keypairs/"${CLOUD_PROVIDER}" ]]; then
      cp "${REPO_ROOT}"/net/keypairs/"${CLOUD_PROVIDER}"/* "${REPO_ROOT}"/net/keypairs/
    fi
  fi

  if [[ "$CLOUD_PROVIDER" = "colo" ]]; then
    execution_step "Stopping Colo nodes before we start"
    "${REPO_ROOT}"/net/net.sh stop
  fi

  execution_step "Starting bootstrap node and ${NUMBER_OF_VALIDATOR_NODES} validator nodes"
  if [[ -n $CHANNEL ]]; then
    # shellcheck disable=SC2068
    # shellcheck disable=SC2086
    "${REPO_ROOT}"/net/net.sh start -t "$CHANNEL" \
      -c idle=$NUMBER_OF_CLIENT_NODES $maybeStartAllowBootFailures \
      --gpu-mode $startGpuMode
  else
    # shellcheck disable=SC2068
    # shellcheck disable=SC2086
    "${REPO_ROOT}"/net/net.sh start -T solana-release*.tar.bz2 \
      -c idle=$NUMBER_OF_CLIENT_NODES $maybeStartAllowBootFailures \
      --gpu-mode $startGpuMode
  fi

  execution_step "Waiting for bootstrap validator's stake to fall below ${BOOTSTRAP_VALIDATOR_MAX_STAKE_THRESHOLD}%"
  wait_for_bootstrap_validator_stake_drop "$BOOTSTRAP_VALIDATOR_MAX_STAKE_THRESHOLD"

  if [[ $NUMBER_OF_CLIENT_NODES -gt 0 ]]; then
    execution_step "Starting ${NUMBER_OF_CLIENT_NODES} client nodes"
    "${REPO_ROOT}"/net/net.sh startclients "$maybeClientOptions" "$CLIENT_OPTIONS"
  fi

  SECONDS=0
  START_SLOT=$(get_slot)
  SLOT_COUNT_START_SECONDS=$SECONDS
  execution_step "Marking beginning of slot rate test - Slot: $START_SLOT, Seconds: $SLOT_COUNT_START_SECONDS"

  if [[ -n $TEST_DURATION_SECONDS ]]; then
    execution_step "Wait ${TEST_DURATION_SECONDS} seconds to complete test"
    sleep "$TEST_DURATION_SECONDS"
  elif [[ "$APPLY_PARTITIONS" = "true" ]]; then
    STATS_START_SECONDS=$SECONDS
    execution_step "Wait $PARTITION_INACTIVE_DURATION before beginning to apply partitions"
    sleep "$PARTITION_INACTIVE_DURATION"
    for (( i=1; i<=PARTITION_ITERATION_COUNT; i++ )); do
      execution_step "Partition Iteration $i of $PARTITION_ITERATION_COUNT"
      execution_step "Applying netem config $NETEM_CONFIG_FILE for $PARTITION_ACTIVE_DURATION seconds"
      "${REPO_ROOT}"/net/net.sh netem --config-file "$NETEM_CONFIG_FILE"
      sleep "$PARTITION_ACTIVE_DURATION"

      execution_step "Resolving partitions for $PARTITION_INACTIVE_DURATION seconds"
      "${REPO_ROOT}"/net/net.sh netem --config-file "$NETEM_CONFIG_FILE" --netem-cmd cleanup
      sleep "$PARTITION_INACTIVE_DURATION"
    done
    STATS_FINISH_SECONDS=$SECONDS
    TEST_DURATION_SECONDS=$((STATS_FINISH_SECONDS - STATS_START_SECONDS))
  else
    # We should never get here
    echo Test duration and partition config not defined
    exit 1
  fi

  END_SLOT=$(get_slot)
  SLOT_COUNT_END_SECONDS=$SECONDS
  execution_step "Marking end of slot rate test - Slot: $END_SLOT, Seconds: $SLOT_COUNT_END_SECONDS"

  SLOTS_PER_SECOND="$(bc <<< "scale=3; ($END_SLOT - $START_SLOT)/($SLOT_COUNT_END_SECONDS - $SLOT_COUNT_START_SECONDS)")"
  execution_step "Average slot rate: $SLOTS_PER_SECOND slots/second over $((SLOT_COUNT_END_SECONDS - SLOT_COUNT_START_SECONDS)) seconds"

  execution_step "Collect statistics about run"
  declare q_mean_tps='
    SELECT ROUND(MEAN("median_sum")) as "mean_tps" FROM (
      SELECT MEDIAN(sum_count) AS "median_sum" FROM (
        SELECT SUM("count") AS "sum_count"
          FROM "'$TESTNET_TAG'"."autogen"."bank-process_transactions"
          WHERE time > now() - '"$TEST_DURATION_SECONDS"'s AND count > 0
          GROUP BY time(1s), host_id)
      GROUP BY time(1s)
    )'

  declare q_max_tps='
    SELECT MAX("median_sum") as "max_tps" FROM (
      SELECT MEDIAN(sum_count) AS "median_sum" FROM (
        SELECT SUM("count") AS "sum_count"
          FROM "'$TESTNET_TAG'"."autogen"."bank-process_transactions"
          WHERE time > now() - '"$TEST_DURATION_SECONDS"'s AND count > 0
          GROUP BY time(1s), host_id)
      GROUP BY time(1s)
    )'

  declare q_mean_confirmation='
    SELECT round(mean("duration_ms")) as "mean_confirmation_ms"
      FROM "'$TESTNET_TAG'"."autogen"."validator-confirmation"
      WHERE time > now() - '"$TEST_DURATION_SECONDS"'s'

  declare q_max_confirmation='
    SELECT round(max("duration_ms")) as "max_confirmation_ms"
      FROM "'$TESTNET_TAG'"."autogen"."validator-confirmation"
      WHERE time > now() - '"$TEST_DURATION_SECONDS"'s'

  declare q_99th_confirmation='
    SELECT round(percentile("duration_ms", 99)) as "99th_percentile_confirmation_ms"
      FROM "'$TESTNET_TAG'"."autogen"."validator-confirmation"
      WHERE time > now() - '"$TEST_DURATION_SECONDS"'s'

  declare q_max_tower_distance_observed='
    SELECT MAX("tower_distance") as "max_tower_distance" FROM (
      SELECT last("slot") - last("root") as "tower_distance"
        FROM "'$TESTNET_TAG'"."autogen"."tower-observed"
        WHERE time > now() - '"$TEST_DURATION_SECONDS"'s
        GROUP BY time(1s), host_id)'

  declare q_last_tower_distance_observed='
      SELECT MEAN("tower_distance") as "last_tower_distance" FROM (
            SELECT last("slot") - last("root") as "tower_distance"
              FROM "'$TESTNET_TAG'"."autogen"."tower-observed"
              GROUP BY host_id)'

  curl -G "${INFLUX_HOST}/query?u=ro&p=topsecret" \
    --data-urlencode "db=${TESTNET_TAG}" \
    --data-urlencode "q=$q_mean_tps;$q_max_tps;$q_mean_confirmation;$q_max_confirmation;$q_99th_confirmation;$q_max_tower_distance_observed;$q_last_tower_distance_observed" |
    python "${REPO_ROOT}"/system-test/testnet-automation-json-parser.py >>"$RESULT_FILE"

  echo "slots_per_second: $SLOTS_PER_SECOND" >>"$RESULT_FILE"

  execution_step "Writing test results to ${RESULT_FILE}"
  RESULT_DETAILS=$(<"$RESULT_FILE")
  upload-ci-artifact "$RESULT_FILE"
}

# shellcheck disable=SC2034
RESULT_DETAILS=
STEP=
execution_step "Initialize Environment"

[[ -n $TESTNET_TAG ]] || TESTNET_TAG=testnet-automation
[[ -n $INFLUX_HOST ]] || INFLUX_HOST=https://metrics.solana.com:8086
[[ -n $BOOTSTRAP_VALIDATOR_MAX_STAKE_THRESHOLD ]] || BOOTSTRAP_VALIDATOR_MAX_STAKE_THRESHOLD=66

if [[ -z $NUMBER_OF_VALIDATOR_NODES ]]; then
  echo NUMBER_OF_VALIDATOR_NODES not defined
  exit 1
fi

startGpuMode="off"
if [[ -z $ENABLE_GPU ]]; then
  ENABLE_GPU=false
fi
if [[ "$ENABLE_GPU" = "true" ]]; then
  maybeEnableGpu="--enable-gpu"
  startGpuMode="on"
fi

if [[ -z $NUMBER_OF_CLIENT_NODES ]]; then
  echo NUMBER_OF_CLIENT_NODES not defined
  exit 1
fi

if [[ -z $SOLANA_METRICS_CONFIG ]]; then
  if [[ -z $SOLANA_METRICS_PARTIAL_CONFIG ]]; then
    echo SOLANA_METRICS_PARTIAL_CONFIG not defined
    exit 1
  fi
  export SOLANA_METRICS_CONFIG="db=$TESTNET_TAG,host=$INFLUX_HOST,$SOLANA_METRICS_PARTIAL_CONFIG"
fi
echo "SOLANA_METRICS_CONFIG: $SOLANA_METRICS_CONFIG"

if [[ -z $ALLOW_BOOT_FAILURES ]]; then
  ALLOW_BOOT_FAILURES=false
fi
if [[ "$ALLOW_BOOT_FAILURES" = "true" ]]; then
  maybeCreateAllowBootFailures="--allow-boot-failures"
  maybeStartAllowBootFailures="-F"
fi

if [[ -z $USE_PUBLIC_IP_ADDRESSES ]]; then
  USE_PUBLIC_IP_ADDRESSES=false
fi
if [[ "$USE_PUBLIC_IP_ADDRESSES" = "true" ]]; then
  maybePublicIpAddresses="-P"
fi

: "${CLIENT_DELAY_START:=0}"

if [[ -z $APPLY_PARTITIONS ]]; then
  APPLY_PARTITIONS=false
fi
if [[ "$APPLY_PARTITIONS" = "true" ]]; then
  if [[ -n $TEST_DURATION_SECONDS ]]; then
    echo Cannot accept TEST_DURATION_SECONDS and a parition looping config
    exit 1
  fi
elif [[ -z $TEST_DURATION_SECONDS ]]; then
  echo TEST_DURATION_SECONDS not defined
  exit 1
fi

if [[ -z $CHANNEL ]]; then
  execution_step "Downloading tar from build artifacts"
  buildkite-agent artifact download "solana-release*.tar.bz2" .
fi

maybeClientOptions=${CLIENT_OPTIONS:+"-c"}
maybeCustomMachineType=${VALIDATOR_NODE_MACHINE_TYPE:+"--custom-machine-type"}

IFS=, read -r -a TESTNET_CLOUD_ZONES <<<"${TESTNET_ZONES}"

RESULT_FILE="$TESTNET_TAG"_SUMMARY_STATS_"$NUMBER_OF_VALIDATOR_NODES".log
rm -f "$RESULT_FILE"

TEST_PARAMS_TO_DISPLAY=(CLOUD_PROVIDER \
                        NUMBER_OF_VALIDATOR_NODES \
                        ENABLE_GPU \
                        VALIDATOR_NODE_MACHINE_TYPE \
                        NUMBER_OF_CLIENT_NODES \
                        CLIENT_OPTIONS \
                        CLIENT_DELAY_START \
                        TESTNET_ZONES \
                        TEST_DURATION_SECONDS \
                        USE_PUBLIC_IP_ADDRESSES \
                        ALLOW_BOOT_FAILURES \
                        ADDITIONAL_FLAGS \
                        APPLY_PARTITIONS \
                        NETEM_CONFIG_FILE \
                        PARTITION_ACTIVE_DURATION \
                        PARTITION_INACTIVE_DURATION \
                        PARTITION_ITERATION_COUNT \
                        )

TEST_CONFIGURATION=
for i in "${TEST_PARAMS_TO_DISPLAY[@]}"; do
  if [[ -n ${!i} ]]; then
    TEST_CONFIGURATION+="${i} = ${!i} | "
  fi
done

# shellcheck disable=SC2034
TESTNET_START_UNIX_MSECS="$(($(date +%s%N)/1000000))"

launch_testnet
