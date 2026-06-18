import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { useUiPreferencesStore } from './useUiPreferencesStore';

type State = {
  selectedOrgId: string | null;
  setSelectedOrgId: (orgId: string | null) => void;
  clearSelectedOrgId: () => void;
};

const ORGANIZATION_SELECTION_KEY_PREFIX = 'organization-selection';
const SIGNED_OUT_SCOPE = 'signed-out';

let activePersistKey: string | null = null;

export const useOrganizationStore = create<State>()(
  persist(
    (set) => ({
      selectedOrgId: null,
      setSelectedOrgId: (orgId) => set({ selectedOrgId: orgId }),
      clearSelectedOrgId: () => set({ selectedOrgId: null }),
    }),
    {
      name: ORGANIZATION_SELECTION_KEY_PREFIX,
      partialize: (state) => ({ selectedOrgId: state.selectedOrgId }),
      skipHydration: true,
    }
  )
);

export function hydrateOrganizationSelectionForUser(userId: string | null) {
  const userScope = userId ?? SIGNED_OUT_SCOPE;
  const nextPersistKey = `${ORGANIZATION_SELECTION_KEY_PREFIX}:${userScope}`;

  if (activePersistKey === nextPersistKey) {
    return;
  }

  activePersistKey = nextPersistKey;
  useOrganizationStore.persist.setOptions({ name: nextPersistKey });
  useOrganizationStore.setState({ selectedOrgId: null });
  void useOrganizationStore.persist.rehydrate();
}

// Sync org store changes into the UI preferences store for server persistence
useOrganizationStore.subscribe((state) => {
  useUiPreferencesStore.getState().setSelectedOrgId(state.selectedOrgId);
});

export const useSelectedOrgId = () =>
  useOrganizationStore((s) => s.selectedOrgId);
export const useSetSelectedOrgId = () =>
  useOrganizationStore((s) => s.setSelectedOrgId);
