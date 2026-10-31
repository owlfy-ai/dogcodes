import { useRef, useState } from "react";
import type { QuestionAnswer, QuestionForm, QuestionSchema } from "@dogcodes/protocol";
import "./question.css";
import { translate as t } from "./i18n";

export function QuestionCard({ form, busy = false, disabled = false, onAnswer, children }: {
  form: QuestionForm; busy?: boolean; disabled?: boolean;
  onAnswer?: (answer: QuestionAnswer) => void; children?: React.ReactNode;
}) {
  const properties = form.schema.properties ?? {};
  const keys = [...new Set([...(form.schema.required ?? []), ...Object.keys(properties)])].filter(key => key in properties);
  const [page, setPage] = useState(0);
  const [customOpen, setCustomOpen] = useState<Record<string, boolean>>({});
  const formRef = useRef<HTMLFormElement>(null);
  const fieldsRef = useRef<HTMLDivElement>(null);
  const activeIndex = Math.min(page, Math.max(0, keys.length - 1));
  const lastPage = activeIndex >= keys.length - 1;
  const goTo = (index: number) => { setPage(index); if (fieldsRef.current) fieldsRef.current.scrollTop = 0; };
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  const set = (key: string, value: unknown) => setValues(old => ({ ...old, [key]: value }));
  const required = new Set(form.schema.required ?? []);
  const result = Object.fromEntries(keys.flatMap(key => {
    const schema = properties[key];
    const text = custom[key]?.trim();
    let value = values[key];
    if (text) value = schema.type === "array" ? [...(Array.isArray(value) ? value : []), text] : text;
    return value === undefined || value === "" ? [] : [[key, value]];
  }));
  const complete = [...required].every(key => result[key] !== undefined && (!Array.isArray(result[key]) || result[key].length > 0));
  const blocked = busy || disabled || !onAnswer;
  const answered = (key: string) => result[key] !== undefined && (!Array.isArray(result[key]) || result[key].length > 0);
  const currentComplete = !required.has(keys[activeIndex]) || answered(keys[activeIndex]);
  const contextTitle = form.title.trim();
  const showContext = contextTitle && !/^(请回答以下问题|请确认以下问题|请回答|请选择|questions?|choose)[。.!！：:]*$/i.test(contextTitle) && contextTitle !== properties[keys[activeIndex]]?.title;
  const validateFields = (indices: number[]) => {
    const fields = formRef.current?.querySelectorAll<HTMLFieldSetElement>(".question-field");
    for (const index of indices) {
      const controls = fields?.[index]?.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("input, select, textarea");
      const invalid = controls && [...controls].find(control => !control.validity.valid);
      if (invalid) {
        goTo(index);
        requestAnimationFrame(() => { invalid.focus(); invalid.reportValidity(); });
        return false;
      }
    }
    return true;
  };
  return <form ref={formRef} noValidate className="approval-card question-card composer-approval composer-question" aria-label={t("question.label")} aria-busy={busy}
    onKeyDown={event => event.stopPropagation()}
    onSubmit={event => {
      event.preventDefault();
      if (blocked) return;
      if (!lastPage) { if (currentComplete && validateFields([activeIndex])) goTo(activeIndex + 1); return; }
      if (complete && validateFields(keys.map((_, index) => index))) onAnswer?.({ cancelled: false, values: result });
    }}>
    <header className="approval-heading"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-8l-5 3v-3H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2Z"/><path d="M9.5 8a2.5 2.5 0 1 1 4.6 1.4L12 11v1m0 2v.5"/></svg><span>{t("question.title")}</span><span className="question-progress">{keys.filter(answered).length === keys.length ? t("question.allAnswered") : `${activeIndex + 1} / ${keys.length}`}</span></header>
    {showContext && <p className="question-context">{contextTitle}</p>}
    {keys.length > 1 && <nav className="question-nav" aria-label={t("question.nav")}>{keys.map((key, index) => <button type="button" key={key} disabled={busy || disabled} aria-current={index === activeIndex ? "step" : undefined} aria-label={t("question.step", { index: index + 1, title: properties[key].title ?? key })} data-tooltip={properties[key].title ?? key} onClick={() => goTo(index)}><span className={`question-step ${answered(key) ? "answered" : ""}`}>{answered(key) ? <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="m3 8 3 3 7-7"/></svg> : index + 1}</span><span>{properties[key].title ?? key}</span></button>)}</nav>}
    <div className="question-fields" ref={fieldsRef}>
      {keys.map((key, index) => {
        const schema = properties[key];
        const item: QuestionSchema = schema.type === "array" ? schema.items ?? {} : schema;
        const options = item["x-options"] ?? item.oneOf ?? item.anyOf ?? item.enum?.map((value, i) => ({ const: value, title: item.enumNames?.[i] ?? String(value) }));
        const multiple = schema.type === "array";
        const other = item["x-allowOther"];
        return <fieldset key={key} disabled={blocked} hidden={index !== activeIndex} className="question-field">
          <legend>{schema.title ?? key}<small>{multiple ? t("question.multiple") : !required.has(key) ? t("question.optional") : options?.length ? t("question.single") : ""}</small></legend>
          {schema.description && <p className="question-description">{schema.description}</p>}
          {options?.length ? <>
            <div className="question-options">{options.map((option, i) => {
              const checked = multiple ? Array.isArray(values[key]) && values[key].includes(option.const) : values[key] === option.const && !custom[key];
              const optionTitle = option.title ?? String(option.const);
              const recommended = /\s*[（(](?:recommended|推荐)[)）]\s*$/i.test(optionTitle);
              const displayTitle = recommended ? optionTitle.replace(/\s*[（(](?:recommended|推荐)[)）]\s*$/i, "") : optionTitle;
              return <label key={i} className={`question-option ${multiple ? "multiple" : "single"} ${checked ? "selected" : ""}`}>
                <input type={multiple ? "checkbox" : "radio"} name={`question-${index}`} checked={checked} onChange={() => {
                  if (!multiple) { set(key, option.const); setCustom(old => ({ ...old, [key]: "" })); }
                  else { const chosen = Array.isArray(values[key]) ? values[key] : []; set(key, checked ? chosen.filter(v => v !== option.const) : [...chosen, option.const]); }
                }} />
                <span className="question-choice-mark" aria-hidden="true"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="m3 8 3 3 7-7"/></svg></span><span className="question-option-copy"><strong>{displayTitle}{recommended && <em className="question-recommended">{t("question.recommended")}</em>}</strong>{"description" in option && typeof option.description === "string" && <small>{option.description}</small>}{"preview" in option && typeof option.preview === "string" && <details><summary>{t("question.details")}</summary><pre>{option.preview}</pre></details>}</span>
              </label>;
            })}</div>
            {other && <div className="question-custom-area"><button type="button" className="question-custom-toggle" aria-expanded={Boolean(customOpen[key])} onClick={() => setCustomOpen(old => ({ ...old, [key]: !old[key] }))}><span aria-hidden="true">{customOpen[key] ? "−" : "+"}</span>{t(custom[key]?.trim() ? "question.otherFilled" : multiple ? "question.addAnswer" : "question.otherAnswer")}</button>{customOpen[key] && <label className="question-custom"><input aria-label={t("question.customLabel", { title: schema.title ?? key })} placeholder={t(multiple ? "question.addPlaceholder" : "question.answerPlaceholder")} value={custom[key] ?? ""} maxLength={10000} onChange={event => setCustom(old => ({ ...old, [key]: event.target.value }))} /></label>}</div>}
          </> : schema.type === "boolean" ? <select aria-label={schema.title ?? key} required={required.has(key)} value={values[key] === undefined ? "" : String(values[key])} onChange={event => set(key, event.target.value === "" ? undefined : event.target.value === "true")}><option value="">{t("question.choose")}</option><option value="true">{t("question.yes")}</option><option value="false">{t("question.no")}</option></select>
            : schema.type === "number" || schema.type === "integer" ? <input aria-label={schema.title ?? key} type="number" step={schema.type === "integer" ? 1 : "any"} min={schema.minimum} max={schema.maximum} required={required.has(key)} value={String(values[key] ?? "")} onChange={event => set(key, event.target.value === "" ? undefined : event.target.valueAsNumber)} />
            : schema.writeOnly ? <input aria-label={schema.title ?? key} type="password" autoComplete="off" required={required.has(key)} value={String(values[key] ?? "")} onChange={event => set(key, event.target.value)} />
            : <textarea aria-label={schema.title ?? key} rows={3} required={required.has(key)} minLength={schema.minLength} maxLength={schema.maxLength ?? 20000} value={String(values[key] ?? "")} onChange={event => set(key, event.target.value)} />}
        </fieldset>;
      })}
    </div>
    {children}
    {onAnswer && <footer className="approval-actions question-actions"><button type="button" className="approval-secondary" disabled={blocked} onClick={() => onAnswer({ cancelled: true, values: {} })}>{t("question.cancel")}</button><span className="question-footer-nav">{activeIndex > 0 && <button type="button" className="question-previous" disabled={blocked} onClick={() => goTo(activeIndex - 1)}>{t("question.previous")}</button>}<button type="submit" className="approval-primary" disabled={blocked || (lastPage ? !complete : !currentComplete)}>{t(busy ? "question.submitting" : lastPage ? "question.submit" : "question.next")}{!lastPage && <span aria-hidden="true"> →</span>}</button></span></footer>}
  </form>;
}
