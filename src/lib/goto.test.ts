import { describe, expect, it } from 'vitest'

import { isPathQuery, resolvePath, splitPath } from './goto'

describe('isPathQuery', () => {
  it('takes an absolute path or a home-relative one as a place', () => {
    expect(isPathQuery('/home/ada/.omp/agent/models.yml', 'remote')).toBe(true)
    expect(isPathQuery('  /etc ', 'remote')).toBe(true)
    expect(isPathQuery('~', 'remote')).toBe(true)
    expect(isPathQuery('~/projects', 'local')).toBe(true)
  })

  it('leaves names alone, including ones that start with a tilde', () => {
    expect(isPathQuery('models', 'remote')).toBe(false)
    expect(isPathQuery('agent/models.yml', 'remote')).toBe(false)
    expect(isPathQuery('~$report.docx', 'local')).toBe(false)
    expect(isPathQuery('~ada', 'remote')).toBe(false)
    expect(isPathQuery('', 'remote')).toBe(false)
  })

  // A server is POSIX; `C:\` there is a name with a colon and a backslash in it.
  it('knows drive letters on the local side only', () => {
    expect(isPathQuery('C:\\Users', 'local')).toBe(true)
    expect(isPathQuery('d:/logs', 'local')).toBe(true)
    expect(isPathQuery('C:\\Users', 'remote')).toBe(false)
  })
})

describe('resolvePath', () => {
  it('drops doubled and trailing slashes', () => {
    expect(resolvePath('//srv///app/', '/home/ada')).toBe('/srv/app')
    expect(resolvePath('/', '/home/ada')).toBe('/')
  })

  // The breadcrumb is built from the string, so a `..` left in draws a `..` crumb.
  it('resolves dot segments, and never climbs above the root', () => {
    expect(resolvePath('/srv/app/../logs/./today', '/')).toBe('/srv/logs/today')
    expect(resolvePath('/../../etc', '/')).toBe('/etc')
  })

  it('expands a tilde against home', () => {
    expect(resolvePath('~', '/home/ada')).toBe('/home/ada')
    expect(resolvePath('~/.ssh/config', '/home/ada/')).toBe('/home/ada/.ssh/config')
    expect(resolvePath('~/x', '/')).toBe('/x')
  })

  it('keeps a POSIX backslash as part of the name', () => {
    expect(resolvePath('/data/a\\b', '/')).toBe('/data/a\\b')
  })

  it('spells a Windows path with backslashes, whichever were typed', () => {
    expect(resolvePath('C:/Users/ada/../bob/', 'C:\\Users\\ada')).toBe('C:\\Users\\bob')
    expect(resolvePath('~/Desktop', 'C:\\Users\\ada')).toBe('C:\\Users\\ada\\Desktop')
    expect(resolvePath('C:\\', 'C:\\Users\\ada')).toBe('C:\\')
  })
})

describe('splitPath', () => {
  it('names the folder a file is in', () => {
    expect(splitPath('/home/ada/.omp/agent/models.yml')).toEqual({
      parent: '/home/ada/.omp/agent',
      name: 'models.yml',
    })
    expect(splitPath('/notes.md')).toEqual({ parent: '/', name: 'notes.md' })
  })

  it('keeps a drive root a root', () => {
    expect(splitPath('C:\\Users\\ada\\notes.md')).toEqual({
      parent: 'C:\\Users\\ada',
      name: 'notes.md',
    })
    expect(splitPath('C:\\notes.md')).toEqual({ parent: 'C:\\', name: 'notes.md' })
  })
})
