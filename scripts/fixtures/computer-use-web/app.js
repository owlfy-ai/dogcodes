const price = 7;
let last = null;
document.querySelector('#calculate').addEventListener('click', () => {
  const quantity = Number(document.querySelector('#quantity').value);
  const total = price + quantity; // Intentional fixture bug, repaired by the agent.
  last = { quantity, total };
  document.querySelector('#total').textContent = `Total: $${total}`;
});
document.querySelector('#save').addEventListener('click', async () => {
  if (!last) return;
  const response = await fetch('/report', { method: 'POST', body: JSON.stringify(last) });
  document.querySelector('#status').textContent = response.ok ? 'Verification saved' : 'Save failed';
});
