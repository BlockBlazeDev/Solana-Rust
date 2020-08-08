import React from "react";
import { StakeAccount } from "solana-sdk-wasm";
import { PublicKey, Connection, StakeProgram } from "@solana/web3.js";
import { useCluster } from "../cluster";
import { HistoryProvider } from "./history";
import { TokensProvider } from "./tokens";
export { useAccountHistory } from "./history";

export enum FetchStatus {
  Fetching,
  FetchFailed,
  Fetched,
}

export interface Details {
  executable: boolean;
  owner: PublicKey;
  space: number;
  data?: StakeAccount;
}

export interface Account {
  pubkey: PublicKey;
  status: FetchStatus;
  lamports?: number;
  details?: Details;
}

type Accounts = { [address: string]: Account };
interface State {
  accounts: Accounts;
  url: string;
}

export enum ActionType {
  Update,
  Fetch,
  Clear,
}

interface Update {
  type: ActionType.Update;
  url: string;
  pubkey: PublicKey;
  data: {
    status: FetchStatus;
    lamports?: number;
    details?: Details;
  };
}

interface Fetch {
  type: ActionType.Fetch;
  url: string;
  pubkey: PublicKey;
}

interface Clear {
  type: ActionType.Clear;
  url: string;
}

type Action = Update | Fetch | Clear;
type Dispatch = (action: Action) => void;

function reducer(state: State, action: Action): State {
  if (action.type === ActionType.Clear) {
    return { url: action.url, accounts: {} };
  } else if (action.url !== state.url) {
    return state;
  }

  switch (action.type) {
    case ActionType.Fetch: {
      const address = action.pubkey.toBase58();
      const account = state.accounts[address];
      if (account) {
        const accounts = {
          ...state.accounts,
          [address]: {
            pubkey: account.pubkey,
            status: FetchStatus.Fetching,
          },
        };
        return { ...state, accounts };
      } else {
        const accounts = {
          ...state.accounts,
          [address]: {
            status: FetchStatus.Fetching,
            pubkey: action.pubkey,
          },
        };
        return { ...state, accounts };
      }
    }

    case ActionType.Update: {
      const address = action.pubkey.toBase58();
      const account = state.accounts[address];
      if (account) {
        const accounts = {
          ...state.accounts,
          [address]: {
            ...account,
            ...action.data,
          },
        };
        return { ...state, accounts };
      }
      break;
    }
  }
  return state;
}

const StateContext = React.createContext<State | undefined>(undefined);
const DispatchContext = React.createContext<Dispatch | undefined>(undefined);

type AccountsProviderProps = { children: React.ReactNode };
export function AccountsProvider({ children }: AccountsProviderProps) {
  const { url } = useCluster();
  const [state, dispatch] = React.useReducer(reducer, {
    url,
    accounts: {},
  });

  // Clear account statuses whenever cluster is changed
  React.useEffect(() => {
    dispatch({ type: ActionType.Clear, url });
  }, [url]);

  return (
    <StateContext.Provider value={state}>
      <DispatchContext.Provider value={dispatch}>
        <TokensProvider>
          <HistoryProvider>{children}</HistoryProvider>
        </TokensProvider>
      </DispatchContext.Provider>
    </StateContext.Provider>
  );
}

async function fetchAccountInfo(
  dispatch: Dispatch,
  pubkey: PublicKey,
  url: string
) {
  dispatch({
    type: ActionType.Fetch,
    pubkey,
    url,
  });

  let fetchStatus;
  let details;
  let lamports;
  try {
    const result = await new Connection(url, "recent").getAccountInfo(pubkey);
    if (result === null) {
      lamports = 0;
    } else {
      lamports = result.lamports;
      let data = undefined;

      // Only save data in memory if we can decode it
      if (result.owner.equals(StakeProgram.programId)) {
        try {
          const wasm = await import("solana-sdk-wasm");
          data = wasm.StakeAccount.fromAccountData(result.data);
        } catch (err) {
          console.error("Unexpected error loading wasm", err);
          // TODO store error state in Account info
        }
      }

      details = {
        space: result.data.length,
        executable: result.executable,
        owner: result.owner,
        data,
      };
    }
    fetchStatus = FetchStatus.Fetched;
  } catch (error) {
    console.error("Failed to fetch account info", error);
    fetchStatus = FetchStatus.FetchFailed;
  }
  const data = { status: fetchStatus, lamports, details };
  dispatch({ type: ActionType.Update, data, pubkey, url });
}

export function useAccounts() {
  const context = React.useContext(StateContext);
  if (!context) {
    throw new Error(`useAccounts must be used within a AccountsProvider`);
  }
  return context;
}

export function useAccountInfo(address: string) {
  const context = React.useContext(StateContext);

  if (!context) {
    throw new Error(`useAccountInfo must be used within a AccountsProvider`);
  }

  return context.accounts[address];
}

export function useFetchAccountInfo() {
  const dispatch = React.useContext(DispatchContext);
  if (!dispatch) {
    throw new Error(
      `useFetchAccountInfo must be used within a AccountsProvider`
    );
  }

  const { url } = useCluster();
  return (pubkey: PublicKey) => {
    fetchAccountInfo(dispatch, pubkey, url);
  };
}
