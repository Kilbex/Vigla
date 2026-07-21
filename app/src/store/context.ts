import type { OpsStore } from "./types";

export type StoreSet = (
  partial:
    | Partial<OpsStore>
    | ((state: OpsStore) => Partial<OpsStore> | OpsStore),
  replace?: false,
) => void;

export type StoreGet = () => OpsStore;

export interface StoreSliceContext {
  set: StoreSet;
  get: StoreGet;
}
