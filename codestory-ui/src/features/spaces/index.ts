export type {
  CreateInvestigationSpaceInput,
  InvestigationSpace,
  UpdateInvestigationSpaceInput,
} from "./types";
export {
  INVESTIGATION_SPACES_STORAGE_KEY,
  createSpace,
  deleteSpace,
  listSpaces,
  loadSpace,
  loadSpaces,
  updateSpace,
} from "./storage";
