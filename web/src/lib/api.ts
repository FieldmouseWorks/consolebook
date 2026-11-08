// Compatibility entrypoint. Domain contracts and calls live under ./api/.
export { ApiError } from './api/transport';
export type { ApiErrorBody } from './api/transport';
export * from './api/session';
export * from './api/programs';
export * from './api/users';
export * from './api/installation';
export * from './api/enrollments';
export * from './api/sessions';
export * from './api/drafts';
export * from './api/records';
export * from './api/summaries';
export * from './api/signoffs';
export * from './api/exports';
