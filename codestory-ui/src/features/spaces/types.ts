export type InvestigationSpace = {
  id: string;
  name: string;
  prompt: string;
  activeGraphId: string | null;
  activeSymbolId: string | null;
  notes?: string;
  createdAt: string;
  updatedAt: string;
  owner: string;
};

export type CreateInvestigationSpaceInput = {
  id?: string;
  name: string;
  prompt: string;
  activeGraphId?: string | null;
  activeSymbolId?: string | null;
  notes?: string;
  owner: string;
};

export type UpdateInvestigationSpaceInput = Partial<
  Omit<InvestigationSpace, "id" | "createdAt" | "updatedAt">
>;
