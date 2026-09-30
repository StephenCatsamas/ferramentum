import assert from 'node:assert/strict';
import test from 'node:test';
import {focusWindow} from './focus.js';

test('activates one exact process and refuses stale, ambiguous, or locked targets', () => {
    const window = {get_pid: () => 42};
    let active = null;
    const options = {
        pid: 42, startTicks: '123', locked: false, windows: [window],
        readStat: () => `42 (foot (test)) S ${Array(18).fill('0').join(' ')} 123`,
        activate: value => { active = value; }, focused: () => active,
    };
    assert.equal(focusWindow(options), 'focused');
    assert.equal(active, window);
    for (const [changes, result] of [
        [{startTicks: '124'}, 'changed'],
        [{windows: [window, window]}, 'ambiguous'],
        [{windows: []}, 'missing'],
        [{locked: true}, 'locked'],
        [{readStat: () => 'broken'}, 'changed'],
        [{pid: -1}, 'changed'],
    ]) {
        active = null;
        assert.equal(focusWindow({...options, ...changes}), result);
        assert.equal(active, null);
    }
    assert.equal(focusWindow({...options, activate: () => {}}), 'unfocused');
});
