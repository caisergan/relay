import { describe, expect, it } from 'vitest'

import { transferPaths } from './transfer'

const REMOTE = '/home/ada/projects'
const LOCAL = '/Users/ada/Documents'

describe('transferPaths', () => {
  describe('with no folder aimed at', () => {
    it('downloads into the directory the local pane is showing', () => {
      expect(
        transferPaths({
          direction: 'down',
          remoteDir: REMOTE,
          localDir: LOCAL,
          name: 'notes.txt',
        }),
      ).toEqual({
        remotePath: '/home/ada/projects/notes.txt',
        localPath: '/Users/ada/Documents/notes.txt',
      })
    })

    it('uploads into the directory the remote pane is showing', () => {
      expect(
        transferPaths({
          direction: 'up',
          remoteDir: REMOTE,
          localDir: LOCAL,
          name: 'notes.txt',
        }),
      ).toEqual({
        remotePath: '/home/ada/projects/notes.txt',
        localPath: '/Users/ada/Documents/notes.txt',
      })
    })
  })

  /** The whole point of the feature, and the half that is easy to write backwards.
   * Only the receiving side moves — the source is where the bytes already are, and no
   * drop can change that. */
  describe('when dropped onto a folder', () => {
    it('moves only the local side of a download', () => {
      expect(
        transferPaths({
          direction: 'down',
          remoteDir: REMOTE,
          localDir: LOCAL,
          name: 'notes.txt',
          intoFolder: 'backups',
        }),
      ).toEqual({
        remotePath: '/home/ada/projects/notes.txt',
        localPath: '/Users/ada/Documents/backups/notes.txt',
      })
    })

    it('moves only the remote side of an upload', () => {
      expect(
        transferPaths({
          direction: 'up',
          remoteDir: REMOTE,
          localDir: LOCAL,
          name: 'notes.txt',
          intoFolder: 'incoming',
        }),
      ).toEqual({
        remotePath: '/home/ada/projects/incoming/notes.txt',
        localPath: '/Users/ada/Documents/notes.txt',
      })
    })

    /** Stated as its own case because it is the failure that would not throw: writing
     * to the right name under the wrong parent, or reading from a directory the file
     * was never in. */
    it('never moves the source side', () => {
      const down = transferPaths({
        direction: 'down',
        remoteDir: REMOTE,
        localDir: LOCAL,
        name: 'a.bin',
        intoFolder: 'anywhere',
      })
      expect(down.remotePath).toBe(`${REMOTE}/a.bin`)

      const up = transferPaths({
        direction: 'up',
        remoteDir: REMOTE,
        localDir: LOCAL,
        name: 'a.bin',
        intoFolder: 'anywhere',
      })
      expect(up.localPath).toBe(`${LOCAL}/a.bin`)
    })
  })

  it('transfers a folder the same way it transfers a file', () => {
    expect(
      transferPaths({
        direction: 'down',
        remoteDir: REMOTE,
        localDir: LOCAL,
        name: 'src',
        intoFolder: 'archive',
      }),
    ).toEqual({
      remotePath: '/home/ada/projects/src',
      localPath: '/Users/ada/Documents/archive/src',
    })
  })

  it('does not double a separator at a root', () => {
    expect(
      transferPaths({
        direction: 'down',
        remoteDir: '/',
        localDir: '/',
        name: 'a.bin',
        intoFolder: 'into',
      }),
    ).toEqual({ remotePath: '/a.bin', localPath: '/into/a.bin' })
  })

  it('treats an absent folder and an empty one alike', () => {
    const plain = transferPaths({
      direction: 'down',
      remoteDir: REMOTE,
      localDir: LOCAL,
      name: 'a.bin',
    })
    expect(
      transferPaths({
        direction: 'down',
        remoteDir: REMOTE,
        localDir: LOCAL,
        name: 'a.bin',
        intoFolder: '',
      }),
    ).toEqual(plain)
  })
})
