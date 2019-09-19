#!/usr/bin/env bash

# |source| this file
#
# Utilities for working with Colo instances
#

declare COLO_TODO_PARALLELIZE=false

__cloud_colo_here="$(dirname "${BASH_SOURCE[0]}")"
# shellcheck source=net/scripts/colo-utils.sh
source "${__cloud_colo_here}/colo-utils.sh"

# Default zone
cloud_DefaultZone() {
  echo "Denver"
}

#
# __cloud_FindInstances
#
# Find instances matching the specified pattern.
#
# For each matching instance, an entry in the `instances` array will be added with the
# following information about the instance:
#   "name:zone:public IP:private IP"
#
# filter   - The instances to filter on
#
# examples:
#   $ __cloud_FindInstances "name=exact-machine-name"
#   $ __cloud_FindInstances "name~^all-machines-with-a-common-machine-prefix"
#
__cloud_FindInstances() {
  declare HOST_NAME IP PRIV_IP STATUS ZONE LOCK_USER INSTNAME INSTANCES_TEXT
  declare filter=$1
  instances=()

  if ! $COLO_TODO_PARALLELIZE; then
    colo_load_resources
    colo_load_availability false
  fi
  INSTANCES_TEXT="$(
    for AVAIL in "${COLO_RES_AVAILABILITY[@]}"; do
      IFS=$'\v' read -r HOST_NAME IP PRIV_IP STATUS ZONE LOCK_USER INSTNAME <<<"$AVAIL"
      if [[ $INSTNAME =~ $filter ]]; then
        IP=$PRIV_IP  # Colo public IPs are firewalled to only allow UDP(8000-10000).  Reuse private IP as public and require VPN
        printf "%-40s | publicIp=%-16s privateIp=%s zone=%s\n" "$INSTNAME" "$IP" "$PRIV_IP" "$ZONE" 1>&2
        echo -e "${INSTNAME}:${IP}:${PRIV_IP}:$ZONE"
      fi
    done | sort -t $'\v' -k1
  )"
  if [[ -n "$INSTANCES_TEXT" ]]; then
    while read -r LINE; do
      instances+=( "$LINE" )
    done <<<"$INSTANCES_TEXT"
  fi
}

#
# cloud_FindInstances [namePrefix]
#
# Find instances with names matching the specified prefix
#
# For each matching instance, an entry in the `instances` array will be added with the
# following information about the instance:
#   "name:public IP:private IP"
#
# namePrefix - The instance name prefix to look for
#
# examples:
#   $ cloud_FindInstances all-machines-with-a-common-machine-prefix
#
cloud_FindInstances() {
  declare filter="^${1}.*"
  __cloud_FindInstances "$filter"
}

#
# cloud_FindInstance [name]
#
# Find an instance with a name matching the exact pattern.
#
# For each matching instance, an entry in the `instances` array will be added with the
# following information about the instance:
#   "name:public IP:private IP"
#
# name - The instance name to look for
#
# examples:
#   $ cloud_FindInstance exact-machine-name
#
cloud_FindInstance() {
  declare name="^${1}$"
  __cloud_FindInstances "$name"
}

#
# cloud_Initialize [networkName]
#
# Perform one-time initialization that may be required for the given testnet.
#
# networkName   - unique name of this testnet
#
# This function will be called before |cloud_CreateInstances|
cloud_Initialize() {
  # networkName=$1 # unused
  # zone=$2 #unused
  colo_load_resources
  if $COLO_TODO_PARALLELIZE; then
    colo_load_availability
  fi
}

#
# cloud_CreateInstances [networkName] [namePrefix] [numNodes] [imageName]
#                       [machineType] [bootDiskSize] [enableGpu]
#                       [startupScript] [address]
#
# Creates one more identical instances.
#
# networkName   - unique name of this testnet
# namePrefix    - unique string to prefix all the instance names with
# numNodes      - number of instances to create
# imageName     - Disk image for the instances
# machineType   - GCE machine type.  Note that this may also include an
#                 `--accelerator=` or other |gcloud compute instances create|
#                 options
# bootDiskSize  - Optional size of the boot disk in GB
# enableGpu     - Optionally enable GPU, use the value "true" to enable
#                 eg, request 4 K80 GPUs with "count=4,type=nvidia-tesla-k80"
# startupScript - Optional startup script to execute when the instance boots
# address       - Optional name of the GCE static IP address to attach to the
#                 instance.  Requires that |numNodes| = 1 and that addressName
#                 has been provisioned in the GCE region that is hosting `$zone`
# bootDiskType  - Optional specify SSD or HDD boot disk
# additionalDiskSize - Optional specify size of additional storage volume
#
# Tip: use cloud_FindInstances to locate the instances once this function
#      returns
cloud_CreateInstances() {
  #declare networkName="$1" # unused
  declare namePrefix="$2"
  declare numNodes="$3"
  #declare enableGpu="$4" # unused
  declare machineType="$5"
  # declare zone="$6" # unused
  #declare optionalBootDiskSize="$7" # unused
  #declare optionalStartupScript="$8" # unused
  #declare optionalAddress="$9" # unused
  #declare optionalBootDiskType="${10}" # unused
  #declare optionalAdditionalDiskSize="${11}" # unused
  declare sshPrivateKey="${12}"

  declare -a nodes
  if [[ $numNodes = 1 ]]; then
    nodes=("$namePrefix")
  else
    for node in $(seq -f "${namePrefix}%0${#numNodes}g" 1 "$numNodes"); do
      nodes+=("$node")
    done
  fi

  if $COLO_TODO_PARALLELIZE; then
    declare HOST_NAME IP PRIV_IP STATUS ZONE LOCK_USER INSTNAME INDEX RES LINE
    declare -a AVAILABLE
    declare AVAILABLE_TEXT
    AVAILABLE_TEXT="$(
      for RES in "${COLO_RES_AVAILABILITY[@]}"; do
        IFS=$'\v' read -r HOST_NAME IP PRIV_IP STATUS ZONE LOCK_USER INSTNAME <<<"$RES"
        if [[ "FREE" = "$STATUS" ]]; then
          INDEX=$(colo_res_index_from_ip "$IP")
          RES_MACH="${COLO_RES_MACHINE[$INDEX]}"
          if colo_machine_types_compatible "$RES_MACH" "$machineType"; then
            if ! colo_node_is_requisitioned "$INDEX" "${COLO_RES_REQUISITIONED[*]}"; then
              echo -e "$RES_MACH\v$IP"
            fi
          fi
        fi
      done | sort -nt $'\v' -k1,1
    )"

    if [[ -n "$AVAILABLE_TEXT" ]]; then
      while read -r LINE; do
        AVAILABLE+=("$LINE")
      done <<<"$AVAILABLE_TEXT"
    fi

    if [[ ${#AVAILABLE[@]} -lt $numNodes ]]; then
      echo "Insufficient resources available to allocate $numNodes $namePrefix" 1>&2
      exit 1
    fi

    declare node
    declare AI=0
    for node in "${nodes[@]}"; do
      IFS=$'\v' read -r _ IP <<<"${AVAILABLE[$AI]}"
      colo_node_requisition "$IP" "$node" >/dev/null
      AI=$((AI+1))
    done
  else
    declare RES_MACH node
    declare RI=0
    declare NI=0
    while [[ $NI -lt $numNodes && $RI -lt $COLO_RES_N ]]; do
      node="${nodes[$NI]}"
      RES_MACH="${COLO_RES_MACHINE[$RI]}"
      IP="${COLO_RES_IP_PRIV[$RI]}"
      if colo_machine_types_compatible "$RES_MACH" "$machineType"; then
        if colo_node_requisition "$IP" "$node" "$sshPrivateKey" >/dev/null; then
          NI=$((NI+1))
        fi
      fi
      RI=$((RI+1))
    done
  fi
}

#
# cloud_DeleteInstances
#
# Deletes all the instances listed in the `instances` array
#
cloud_DeleteInstances() {
  declare _ IP _ _
  for instance in "${instances[@]}"; do
    IFS=':' read -r _ IP _ _ <<< "$instance"
    colo_node_free "$IP" >/dev/null
  done
}

#
# cloud_WaitForInstanceReady [instanceName] [instanceIp] [instanceZone] [timeout]
#
# Return once the newly created VM instance is responding.  This function is cloud-provider specific.
#
cloud_WaitForInstanceReady() {
  #declare instanceName="$1" # unused
  declare instanceIp="$2"
  declare timeout="$4"

  timeout "${timeout}"s bash -c "set -o pipefail; until ping -c 3 $instanceIp | tr - _; do echo .; done"
}

#
# cloud_FetchFile [instanceName] [publicIp] [remoteFile] [localFile]
#
# Fetch a file from the given instance.  This function uses a cloud-specific
# mechanism to fetch the file
#
cloud_FetchFile() {
  #declare instanceName="$1" # unused
  declare publicIp="$2"
  declare remoteFile="$3"
  declare localFile="$4"
  #declare zone="$5" # unused
  scp \
    -o "StrictHostKeyChecking=no" \
    -o "UserKnownHostsFile=/dev/null" \
    -o "User=solana" \
    -o "LogLevel=ERROR" \
    -F /dev/null \
    "solana@$publicIp:$remoteFile" "$localFile"
}

cloud_StatusAll() {
  declare HOST_NAME IP PRIV_IP STATUS ZONE LOCK_USER INSTNAME
  if ! $COLO_TODO_PARALLELIZE; then
    colo_load_resources
    colo_load_availability false
  fi
  for AVAIL in "${COLO_RES_AVAILABILITY[@]}"; do
    IFS=$'\v' read -r HOST_NAME IP PRIV_IP STATUS ZONE LOCK_USER INSTNAME <<<"$AVAIL"
    printf "%-30s | publicIp=%-16s privateIp=%s status=%s who=%s zone=%s inst=%s\n" "$HOST_NAME" "$IP" "$PRIV_IP" "$STATUS" "$LOCK_USER" "$ZONE" "$INSTNAME"
  done
}
