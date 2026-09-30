export function focusWindow({pid, startTicks, locked, windows, readStat, activate, focused}) {
    if (locked)
        return 'locked';
    if (!Number.isInteger(pid) || pid <= 0 || !/^\d+$/.test(startTicks))
        return 'changed';
    try {
        const stat = readStat(pid);
        const fields = stat.slice(stat.lastIndexOf(')') + 1).trim().split(/\s+/);
        if (Number(stat.slice(0, stat.indexOf('(')).trim()) !== pid ||
            fields[19] !== startTicks || ['Z', 'X'].includes(fields[0]))
            return 'changed';
        const matches = windows.filter(window => window.get_pid() === pid);
        if (matches.length > 1)
            return 'ambiguous';
        if (!matches.length)
            return 'missing';
        activate(matches[0]);
        return focused() === matches[0] ? 'focused' : 'unfocused';
    } catch {
        return 'changed';
    }
}
