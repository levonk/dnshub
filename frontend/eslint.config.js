import antfu from 'eslint-config-antfu';

export default antfu({
  react: true,
  typescript: true,
  // NextJS app router uses client components and default exports.
  rules: {
    'react/react-in-jsx-scope': 'off',
    'react/no-unescaped-entities': 'off',
    'ts/no-unused-vars': ['warn', { argsIgnorePattern: '^_' }],
  },
  ignores: [
    'out/**',
    '.next/**',
    'node_modules/**',
  ],
});
