/**
 * This file is used for local starting of the app without any development intentions.
 */

import { MinaEnv } from '@shared/types/core/environment/mina-env.type';

export const environment: Readonly<MinaEnv> = {
  production: false,
  identifier: 'Local FE',
  canAddNodes: true,
  globalConfig: {
    features: {
      dashboard: [],
      nodes: ['overview', 'live', 'bootstrap'],
      state: ['actions'],
      snarks: ['scan-state', 'work-pool'],
      mempool: [],
      'block-production': ['won-slots'],
    },
  },
  configs: [
    {
      name: 'Local rust node',
      url: 'http://127.0.0.1:3000',
    },
  ],
};

