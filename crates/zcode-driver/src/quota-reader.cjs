// Subscription windows reported by the native account monitor.
function windowsFrom(data) {
  if (!Array.isArray(data?.limits)) throw new Error('invalid_payload');
  const windows = [];
  for (const limit of data.limits) {
    let minutes;
    if (['TOKENS_LIMIT', 'CREDIT_LIMIT'].includes(limit.type) && limit.unit === 3 && limit.number === 5) minutes = 300;
    else if (['TOKENS_LIMIT', 'CREDIT_LIMIT'].includes(limit.type) && limit.unit === 6 && limit.number === 1) minutes = 10080;
    else continue;
    const percent = limit.percentage;
    if (percent != null && (!Number.isFinite(percent) || percent < 0 || percent > 100)) throw new Error('invalid_payload');
    const reset = limit.nextResetTime;
    if (reset != null && (!Number.isSafeInteger(reset) || reset < 0)) throw new Error('invalid_payload');
    windows.push({ id: `${limit.type}:${limit.unit}:${limit.number}`, scope: null, windowMinutes: minutes,
      usedPercent: percent ?? null, resetsAtUnix: reset == null ? null : Math.floor(reset / 1000) });
  }
  if (!windows.length) throw new Error('invalid_payload');
  return windows.sort((a, b) => a.windowMinutes - b.windowMinutes);
}

module.exports = { windowsFrom };
