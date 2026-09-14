/** Props for a field whose text is exact: a filter, a path, a host, a user name.
 *
 * WebKit on macOS runs the system's text services over every text field it is not told
 * to leave alone — spelling, autocorrection, and a capitalisation suggestion that floats
 * under the caret offering to turn `d` into `D`. In a field whose text is matched
 * against file names or connected to, each of those is a wrong answer offered at the
 * moment of typing, and a hostname corrected into a dictionary word fails to resolve.
 * `writingsuggestions` switches off the inline predictions newer WebKit adds on top. */
export const exactText = {
  autoComplete: 'off',
  autoCorrect: 'off',
  autoCapitalize: 'off',
  spellCheck: false,
  writingsuggestions: 'false',
} as const
