import { useI18nStore } from '@/stores/i18n'

const ar = {
  accountTitle: 'حسابي في المنصة',
  login: 'تسجيل الدخول',
  register: 'إنشاء حساب',
  logout: 'تسجيل الخروج',
  email: 'البريد الإلكتروني',
  password: 'كلمة المرور',
  passwordHint: '8 أحرف على الأقل',
  fullName: 'الاسم الكامل',
  accountRole: 'نوع الحساب',
  roleStudent: 'طالب',
  roleTeacher: 'معلم',
  roleAdmin: 'مدير المنصة',
  institutionType: 'جهة الدراسة / التدريس',
  typeSchool: 'مدارس',
  typeInstitute: 'معاهد',
  typeUniversity: 'جامعات',
  noAccount: 'ليس لديك حساب؟',
  haveAccount: 'لديك حساب بالفعل؟',
  teacherNotice: 'حساب المعلم يحتاج إلى اعتماد من مدير المنصة قبل أن تتمكن من النشر.',
  checkEmail: 'تم إنشاء الحساب. تحقق من بريدك الإلكتروني لتأكيده ثم سجّل الدخول.',
  invalidCredentials: 'البريد الإلكتروني أو كلمة المرور غير صحيحة.',
  genericError: 'حدث خطأ، حاول مرة أخرى.',
  platformOff: 'خدمة الحسابات غير مفعّلة في هذه النسخة.',
  pendingTitle: 'حسابك قيد المراجعة',
  pendingDesc: 'سيراجع مدير المنصة طلبك قريباً. ستتمكن من النشر فور اعتماد حسابك.',
  rejectedTitle: 'تم رفض الطلب',
  suspendedTitle: 'الحساب موقوف',
  reason: 'السبب',
  platformHome: 'المنصة التعليمية',
  welcome: 'مرحباً،',
  platformHomeSoon: 'ستظهر هنا المدارس والمعاهد والجامعات في المرحلة القادمة.',
  mineAccountDesc: 'تسجيل الدخول والتسجيل في المنصة التعليمية',
}

const en: typeof ar = {
  accountTitle: 'My platform account',
  login: 'Sign in',
  register: 'Create account',
  logout: 'Sign out',
  email: 'Email',
  password: 'Password',
  passwordHint: 'At least 8 characters',
  fullName: 'Full name',
  accountRole: 'Account type',
  roleStudent: 'Student',
  roleTeacher: 'Teacher',
  roleAdmin: 'Platform admin',
  institutionType: 'Institution type',
  typeSchool: 'Schools',
  typeInstitute: 'Institutes',
  typeUniversity: 'Universities',
  noAccount: "Don't have an account?",
  haveAccount: 'Already have an account?',
  teacherNotice: 'Teacher accounts must be approved by a platform admin before you can publish.',
  checkEmail: 'Account created. Check your email to confirm it, then sign in.',
  invalidCredentials: 'Incorrect email or password.',
  genericError: 'Something went wrong, please try again.',
  platformOff: 'Accounts are not enabled in this build.',
  pendingTitle: 'Your account is under review',
  pendingDesc: 'A platform admin will review your request soon. You can publish once approved.',
  rejectedTitle: 'Request rejected',
  suspendedTitle: 'Account suspended',
  reason: 'Reason',
  platformHome: 'Learning platform',
  welcome: 'Welcome,',
  platformHomeSoon: 'Schools, institutes and universities will appear here in the next phase.',
  mineAccountDesc: 'Sign in or register on the learning platform',
}

export type PlatformKey = keyof typeof ar

/** Platform strings: Arabic for `ar`, English for every other locale (until translated). */
export function usePt() {
  const i18n = useI18nStore()
  return (key: PlatformKey): string => (i18n.locale === 'ar' ? ar : en)[key]
}
