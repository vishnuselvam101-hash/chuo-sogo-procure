import { createContext, useContext } from "react";
import { bridge, type Bridge } from "./bridge";

const BridgeContext = createContext<Bridge>(bridge);

export const BridgeProvider = BridgeContext.Provider;

export function useBridge(): Bridge {
  return useContext(BridgeContext);
}
