export type UiLanguage =
  | "system"
  | "zh-CN"
  | "zh-TW"
  | "en"
  | "ja"
  | "ko"
  | "fr"
  | "de"
  | "es"
  | "pt-BR"
  | "ru"
  | "it";

export type ResolvedLanguage = Exclude<UiLanguage, "system">;

export interface LanguageInfo {
  id: UiLanguage;
  label: string;
  nativeLabel: string;
}

export interface TranslationDict {
  common: {
    systemDefault: string;
    light: string;
    dark: string;
    save: string;
    saving: string;
    cancel: string;
    confirm: string;
    delete: string;
    retry: string;
    close: string;
    copy: string;
    copied: string;
    loading: string;
    error: string;
  };
  preferences: {
    title: string;
    open: string;
    language: string;
    theme: string;
    scale: string;
    usage: string;
    upgrade: string;
    disconnect: string;
    system: string;
    zoomIn: string;
    zoomOut: string;
    actualSize: string;
  };
  languages: Record<UiLanguage, string>;
  nav: {
    newConversation: string;
    creating: string;
    connectAgent: string;
    agentManagement: string;
    mainNav: string;
    toggleSidebar: string;
    history: string;
    pin: string;
    unpin: string;
    archive: string;
    rename: string;
    deleteTask: string;
    newProject: string;
    createProject: string;
    editProject: string;
    sortBy: string;
    sectionSortBy: string;
    recentUpdated: string;
    manualSort: string;
    pinnedSection: string;
    projectsSection: string;
    archivedSection: string;
  };
  header: {
    temporaryWorkspace: string;
    toggleTerminal: string;
    collapseTerminal: string;
    toggleReview: string;
    detachedReview: string;
  };
  welcome: {
    greeting: string;
    readyAgentSelected: string;
    noAgentAvailable: string;
    dropFiles: string;
    retry: string;
    openFolder: string;
    selectProject: string;
  };
  composer: {
    placeholder: string;
    addAttachment: string;
    send: string;
    stop: string;
    localExecution: string;
    selectAgent: string;
    nextRoundAgent: string;
    currentAgent: string;
    quotaExhausted: string;
    tooltipShortcuts: string;
    enableInManagement: string;
    stopPending: string;
  };
  turn: {
    copy: string;
    copied: string;
    undo: string;
    fork: string;
    thinking: string;
    duration: string;
    tokens: string;
  };
  terminal: {
    title: string;
    projectTerminal: string;
    starting: string;
    connecting: string;
    adjustHeight: string;
    hideOutput: string;
    newTerminal: string;
    closeTerminal: string;
    reconnecting: string;
  };
  review: {
    changes: string;
    stage: string;
    unstage: string;
    discard: string;
    commitAndPush: string;
    generateCommit: string;
    selectCommitAgent: string;
    selectCommitLanguage: string;
    commitLanguage: string;
    allFiles: string;
  };
  agentManagement: {
    title: string;
    description: string;
    refreshQuota: string;
    refreshing: string;
    installedCount: string;
    quotaCalculation: string;
    refreshSuccess: string;
    refreshFailed: string;
    fetchingList: string;
    noRefreshable: string;
    progress: string;
  };
  agentWelcome: {
    title: string;
    continue: string;
    notInstalled: string;
    ready: string;
    loggedIn: string;
    connected: string;
    testing: string;
    demoData: string;
    authPending: string;
  };
}
