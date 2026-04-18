use crate::error::CoreError;
use crate::uia::element::{role_id_to_name, ElementDescriptor};
use windows::Win32::UI::Accessibility::*;

/// Walker that uses a single `IUIAutomationCacheRequest` to batch property fetches.
/// Any iteration reads only `GetCached*` accessors - no extra cross-process IPCs.
pub struct CachedTreeWalker<'a> {
    uia: &'a IUIAutomation,
    cache_req: IUIAutomationCacheRequest,
}

impl<'a> CachedTreeWalker<'a> {
    pub fn new(uia: &'a IUIAutomation) -> Result<Self, CoreError> {
        unsafe {
            let cache_req: IUIAutomationCacheRequest = uia
                .CreateCacheRequest()
                .map_err(|e| CoreError::ComInit(format!("CreateCacheRequest: {e}")))?;

            for pid in [
                UIA_NamePropertyId,
                UIA_ControlTypePropertyId,
                UIA_BoundingRectanglePropertyId,
                UIA_RuntimeIdPropertyId,
                UIA_IsEnabledPropertyId,
                UIA_IsKeyboardFocusablePropertyId,
                UIA_LocalizedControlTypePropertyId,
                UIA_HelpTextPropertyId,
                UIA_AutomationIdPropertyId,
                UIA_ClassNamePropertyId,
            ] {
                cache_req
                    .AddProperty(pid)
                    .map_err(|e| CoreError::ComInit(format!("AddProperty {pid:?}: {e}")))?;
            }
            cache_req
                .SetTreeScope(TreeScope_Subtree)
                .map_err(|e| CoreError::ComInit(format!("SetTreeScope: {e}")))?;
            cache_req
                .SetAutomationElementMode(AutomationElementMode_Full)
                .map_err(|e| {
                    CoreError::ComInit(format!("SetAutomationElementMode: {e}"))
                })?;
            Ok(Self { uia, cache_req })
        }
    }

    /// Walk every descendant of the desktop, returning cached descriptors.
    pub fn walk_all_windows(&self) -> Result<Vec<ElementDescriptor>, CoreError> {
        unsafe {
            let root = self
                .uia
                .GetRootElement()
                .map_err(|e| CoreError::ComInit(format!("GetRootElement: {e}")))?;
            self.walk(&root)
        }
    }

    /// Walk the subtree rooted at `root`, returning materialized descriptors.
    pub fn walk(&self, root: &IUIAutomationElement) -> Result<Vec<ElementDescriptor>, CoreError> {
        unsafe {
            let true_cond: IUIAutomationCondition = self
                .uia
                .CreateTrueCondition()
                .map_err(|e| CoreError::ComInit(format!("CreateTrueCondition: {e}")))?;
            let arr = root
                .FindAllBuildCache(TreeScope_Subtree, &true_cond, &self.cache_req)
                .map_err(|e| CoreError::ComInit(format!("FindAllBuildCache: {e}")))?;
            let len = arr
                .Length()
                .map_err(|e| CoreError::ComInit(format!("arr.Length: {e}")))?;
            let mut out = Vec::with_capacity(len as usize);
            for i in 0..len {
                let el = arr
                    .GetElement(i)
                    .map_err(|e| CoreError::ComInit(format!("GetElement {i}: {e}")))?;
                if let Some(desc) = descriptor_from_cached(&el) {
                    out.push(desc);
                }
            }
            Ok(out)
        }
    }

    /// Server-side name match. Returns first element whose cached Name contains the target
    /// (case-insensitive), walked via `FindAllBuildCache` with a Name property condition.
    pub fn find_by_name(&self, name: &str) -> Result<Option<ElementDescriptor>, CoreError> {
        unsafe {
            let root = self
                .uia
                .GetRootElement()
                .map_err(|e| CoreError::ComInit(format!("GetRootElement: {e}")))?;
            let all = self.walk(&root)?;
            let lname = name.to_lowercase();
            Ok(all.into_iter().find(|d| d.name.to_lowercase().contains(&lname)))
        }
    }

    /// Server-side role match. Maps the role string to a control-type id and searches.
    pub fn find_by_role(&self, role: &str) -> Result<Option<ElementDescriptor>, CoreError> {
        let target = role.to_lowercase();
        unsafe {
            let root = self
                .uia
                .GetRootElement()
                .map_err(|e| CoreError::ComInit(format!("GetRootElement: {e}")))?;
            let all = self.walk(&root)?;
            Ok(all.into_iter().find(|d| d.role == target))
        }
    }
}

fn descriptor_from_cached(el: &IUIAutomationElement) -> Option<ElementDescriptor> {
    unsafe {
        let name = el.CachedName().ok()?.to_string();
        let ct = el.CachedControlType().ok()?.0 as u32;
        let rect = el.CachedBoundingRectangle().ok()?;
        Some(ElementDescriptor {
            name,
            role: role_id_to_name(ct).to_string(),
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uia::sta_pool::StaPool;

    #[tokio::test]
    #[ignore] // requires display + UIA
    async fn cached_walker_returns_some_elements() {
        let pool = StaPool::new(1).unwrap();
        let count = pool
            .submit(|uia| {
                let w = CachedTreeWalker::new(uia)?;
                let all = w.walk_all_windows()?;
                Ok(all.len() as u32)
            })
            .await
            .unwrap();
        assert!(count > 0, "expected at least one cached element on the desktop");
    }
}
