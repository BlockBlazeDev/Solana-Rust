import React from "react";
import {
  useNetwork,
  useNetworkDispatch,
  updateNetwork,
  NetworkStatus,
  networkUrl,
  networkName,
  NETWORKS,
  Network
} from "../providers/network";
import { assertUnreachable } from "../utils";

type Props = {
  show: boolean;
  onClose: () => void;
};

function NetworkModal({ show, onClose }: Props) {
  return (
    <div
      className={`modal fade fixed-right${show ? " show" : ""}`}
      onClick={onClose}
    >
      <div className="modal-dialog modal-dialog-vertical">
        <div className="modal-content">
          <div className="modal-body" onClick={e => e.stopPropagation()}>
            <span className="close" onClick={onClose}>
              &times;
            </span>

            <h2 className="text-center mb-4 mt-4">Choose a Cluster</h2>

            <NetworkToggle />
          </div>
        </div>
      </div>
    </div>
  );
}

type InputProps = { activeSuffix: string; active: boolean };
function CustomNetworkInput({ activeSuffix, active }: InputProps) {
  const { customUrl } = useNetwork();
  const dispatch = useNetworkDispatch();
  const [editing, setEditing] = React.useState(false);

  const customClass = (prefix: string) =>
    active ? `${prefix}-${activeSuffix}` : "";

  const inputTextClass = editing ? "" : "text-muted";
  return (
    <div
      className="btn input-group input-group-merge p-0"
      onClick={() =>
        !active && updateNetwork(dispatch, Network.Custom, customUrl)
      }
    >
      <input
        type="text"
        defaultValue={customUrl}
        className={`form-control form-control-prepended ${inputTextClass} ${customClass(
          "border"
        )}`}
        onFocus={() => setEditing(true)}
        onBlur={() => setEditing(false)}
        onInput={e =>
          updateNetwork(dispatch, Network.Custom, e.currentTarget.value)
        }
      />
      <div className="input-group-prepend">
        <div className={`input-group-text pr-0 ${customClass("border")}`}>
          <span className={customClass("text") || "text-dark"}>Custom</span>
        </div>
      </div>
    </div>
  );
}

function NetworkToggle() {
  const { status, network, customUrl } = useNetwork();
  const dispatch = useNetworkDispatch();

  let activeSuffix = "";
  switch (status) {
    case NetworkStatus.Connected:
      activeSuffix = "success";
      break;
    case NetworkStatus.Connecting:
      activeSuffix = "warning";
      break;
    case NetworkStatus.Failure:
      activeSuffix = "danger";
      break;
    default:
      assertUnreachable(status);
  }

  return (
    <div className="btn-group-toggle d-flex flex-wrap mb-4">
      {NETWORKS.map((net, index) => {
        const active = net === network;
        if (net === Network.Custom)
          return (
            <CustomNetworkInput
              key={index}
              activeSuffix={activeSuffix}
              active={active}
            />
          );

        const btnClass = active
          ? `border-${activeSuffix} text-${activeSuffix}`
          : "btn-white text-dark";

        return (
          <label
            key={index}
            className={`btn text-left col-12 mb-3 ${btnClass}`}
          >
            <input
              type="radio"
              checked={active}
              onChange={() => updateNetwork(dispatch, net, customUrl)}
            />
            {`${networkName(net)}`}
            <h6 className="text-muted">{networkUrl(net, customUrl)}</h6>
          </label>
        );
      })}
    </div>
  );
}

export default NetworkModal;
